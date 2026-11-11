//! The garbage collector (JS-03): a nonmoving, stop-the-world mark/sweep
//! over the heap's typed arenas (ADR 0015, ADR 0016).
//!
//! Collection runs only at safe points the embedder chooses: an
//! [`ObjectId`] or other handle held in a Rust local is not a root, so
//! allocation never collects on its own. Callers poll
//! [`Agent::should_collect`] and pass what they hold in [`Roots`]; the
//! interpreter (JS-04) does this between instructions with its registers
//! and frames. Handles a host keeps across safe points (DOM wrappers,
//! callbacks) go in the generation-checked [`HostRoot`] table instead.
//!
//! Marking uses an explicit worklist, so a million-deep prototype or
//! environment chain cannot overflow the stack. Mark bits live in
//! temporary bit sets (one bit per cell) rather than in every cell. Sweep
//! frees unmarked cells onto free lists, so live indices never change,
//! drops freed shapes from the transition tree and freed strings from the
//! atom table, returns emptied arena tails to the allocator, and resets the
//! byte budget to the live footprint.

use super::agent::Agent;
use super::env::EnvId;
use super::heap::{ATOM_BYTES, Heap, OutOfMemory, Symbol};
use super::object::Object;
use super::shape::{PropertyKey, ROOT_SHAPE};
use super::string::JsStr;
use super::value::{Kind, StringId, Value};
use crate::runtime::bigint::BigInt;

/// One bit per arena index.
#[derive(Clone, Debug, Default)]
pub(crate) struct BitSet {
    words: Vec<u64>,
}

impl BitSet {
    pub(crate) fn with_capacity(bits: usize) -> Self {
        Self {
            words: vec![0; bits.div_ceil(64)],
        }
    }

    pub(crate) fn get(&self, index: u32) -> bool {
        let i = index as usize;
        self.words
            .get(i / 64)
            .is_some_and(|w| w & (1 << (i % 64)) != 0)
    }

    /// Set the bit; `true` if it was clear.
    pub(crate) fn insert(&mut self, index: u32) -> bool {
        let i = index as usize;
        if i / 64 >= self.words.len() {
            self.words.resize(i / 64 + 1, 0);
        }
        let word = &mut self.words[i / 64];
        let bit = 1 << (i % 64);
        let fresh = *word & bit == 0;
        *word |= bit;
        fresh
    }
}

/// What a heap record reports to the collector.
pub(crate) trait Tracer {
    fn value(&mut self, value: Value);
    fn string(&mut self, id: StringId);
    fn env(&mut self, id: EnvId);

    fn env_opt(&mut self, id: Option<EnvId>) {
        if let Some(id) = id {
            self.env(id);
        }
    }

    fn key(&mut self, key: PropertyKey) {
        match key {
            PropertyKey::Index(_) => {}
            PropertyKey::String(id) => self.string(id),
            PropertyKey::Symbol(id) => self.value(Value::symbol(id)),
        }
    }
}

/// Handles the caller holds at a safe point, beyond the agent's own
/// intrinsics and the host root table.
#[derive(Clone, Copy, Debug, Default)]
pub struct Roots<'a> {
    /// Values on the caller's stack.
    pub values: &'a [Value],
    /// Environments on the caller's stack (running scopes, the realm's
    /// global environment).
    pub envs: &'a [EnvId],
}

/// A value the host keeps alive across collections. The generation makes
/// a handle that outlives [`Agent::unroot`] read as gone instead of naming
/// whatever reuses its slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HostRoot {
    index: u32,
    generation: u32,
}

/// Budget charge per host root.
const HOST_ROOT_BYTES: usize = 16;

#[derive(Debug, Default)]
pub(crate) struct HostRoots {
    entries: Vec<(u32, Option<Value>)>,
    free: Vec<u32>,
    live: usize,
}

impl HostRoots {
    fn values(&self) -> impl Iterator<Item = Value> + '_ {
        self.entries.iter().filter_map(|(_, v)| *v)
    }
}

/// What one collection did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GcStats {
    /// Bytes charged before the collection.
    pub bytes_before: usize,
    /// Bytes charged after it: the live footprint.
    pub bytes_after: usize,
    /// Objects freed.
    pub objects_freed: usize,
    /// Strings freed, atoms included.
    pub strings_freed: usize,
    /// Symbols freed.
    pub symbols_freed: usize,
    /// BigInts freed.
    pub bigints_freed: usize,
    /// Environment records freed.
    pub envs_freed: usize,
    /// Shapes freed.
    pub shapes_freed: usize,
}

/// The smallest heap growth between collections: below this, collecting
/// costs more than the memory it could return.
pub const MIN_GC_THRESHOLD: usize = 1024 * 1024;

pub(crate) fn initial_threshold(limit: usize) -> usize {
    MIN_GC_THRESHOLD.min(limit)
}

/// The next trigger: twice the live footprint (at least
/// [`MIN_GC_THRESHOLD`] above it), and never past seven eighths of the
/// limit, so a heap near its limit collects before allocation fails.
fn next_threshold(live: usize, limit: usize) -> usize {
    let grown = live.saturating_add(live.max(MIN_GC_THRESHOLD));
    grown.min(limit / 8 * 7).max(live)
}

#[derive(Default)]
struct Marks {
    strings: BitSet,
    symbols: BitSet,
    bigints: BitSet,
    objects: BitSet,
    envs: BitSet,
    shapes: BitSet,
}

enum Gray {
    Object(u32),
    Env(u32),
}

struct Marker<'h> {
    heap: &'h Heap,
    marks: Marks,
    work: Vec<Gray>,
    /// Scratch for keys of newly marked shapes.
    keys: Vec<PropertyKey>,
}

impl Tracer for Marker<'_> {
    fn value(&mut self, value: Value) {
        match value.kind() {
            Kind::String(id) => self.string(id),
            Kind::Symbol(id) => {
                if self.marks.symbols.insert(id.0) {
                    if let Some(description) = self.heap.symbol(id).description {
                        self.string(description);
                    }
                }
            }
            Kind::BigInt(id) => {
                self.marks.bigints.insert(id.0);
            }
            Kind::Object(id) => {
                if self.marks.objects.insert(id.0) {
                    self.work.push(Gray::Object(id.0));
                }
            }
            Kind::Undefined
            | Kind::Null
            | Kind::Boolean(_)
            | Kind::Number(_)
            | Kind::Uninitialized => {}
        }
    }

    fn string(&mut self, id: StringId) {
        self.marks.strings.insert(id.0);
    }

    fn env(&mut self, id: EnvId) {
        if self.marks.envs.insert(id.0) {
            self.work.push(Gray::Env(id.0));
        }
    }
}

impl Marker<'_> {
    fn drain(&mut self) {
        while let Some(gray) = self.work.pop() {
            let heap = self.heap;
            match gray {
                Gray::Object(index) => self.trace_object(heap.objects.get(index)),
                Gray::Env(index) => heap.envs.get(index).trace(self),
            }
        }
    }

    fn trace_object(&mut self, object: &Object) {
        if let Some(proto) = object.proto() {
            self.value(Value::object(proto));
        }
        let mut keys = std::mem::take(&mut self.keys);
        self.heap
            .shapes
            .mark_chain(object.shape, &mut self.marks.shapes, |k| keys.push(k));
        for key in keys.drain(..) {
            self.key(key);
        }
        self.keys = keys;
        for &v in object.slots.iter().chain(&object.elements) {
            self.value(v);
        }
        if let Some(extra) = &object.extra {
            if let Some(dictionary) = &extra.dictionary {
                for (key, stored) in dictionary.entries.iter().flatten() {
                    self.key(*key);
                    self.value(stored.first);
                    self.value(stored.second);
                }
            }
            if let Some(sparse) = &extra.sparse {
                for stored in sparse.values() {
                    self.value(stored.first);
                    self.value(stored.second);
                }
            }
            if let Some(primitive) = extra.primitive {
                self.value(primitive);
            }
        }
    }
}

/// Bytes an object is charged for: the header, its slots and elements,
/// and the out-of-line dictionary and sparse entries.
fn object_footprint(object: &Object) -> usize {
    let mut bytes =
        std::mem::size_of::<Object>() + 8 * (object.slots.len() + object.elements.len());
    if let Some(extra) = &object.extra {
        if let Some(d) = &extra.dictionary {
            bytes += 48 * (d.entries.len() - d.holes as usize);
        }
        if let Some(sparse) = &extra.sparse {
            bytes += 48 * sparse.len();
        }
    }
    bytes
}

/// Bytes the live heap is charged for, computed from scratch.
fn live_footprint(heap: &Heap) -> usize {
    let strings: usize = heap
        .strings
        .iter()
        .map(|(_, s)| std::mem::size_of::<JsStr>() + s.storage_bytes())
        .sum();
    let atoms = (heap.atoms.len() + heap.atom_next.len()) * ATOM_BYTES;
    let symbols = heap.symbols.live() * std::mem::size_of::<Symbol>();
    let bigints: usize = heap
        .bigints
        .iter()
        .map(|(_, b)| std::mem::size_of::<BigInt>() + b.storage_bytes())
        .sum();
    let objects: usize = heap.objects.iter().map(|(_, o)| object_footprint(o)).sum();
    let envs: usize = heap.envs.iter().map(|(_, e)| e.footprint()).sum();
    let shapes = (heap.shapes.len() - 1) * 64;
    let roots = heap.roots.live * HOST_ROOT_BYTES;
    strings + atoms + symbols + bigints + objects + envs + shapes + roots
}

/// Rebuild the atom table without freed strings. Chains keep their order,
/// so lookups behave as before.
fn sweep_atoms(heap: &mut Heap, marks: &BitSet) {
    let heads = std::mem::take(&mut heap.atoms);
    let mut next = std::mem::take(&mut heap.atom_next);
    for (hash, head) in heads {
        let mut last: Option<StringId> = None;
        let mut current = Some(head);
        while let Some(id) = current {
            current = next.remove(&id);
            if !marks.get(id.0) {
                continue;
            }
            match last {
                None => heap.atoms.insert(hash, id),
                Some(prev) => heap.atom_next.insert(prev, id),
            };
            last = Some(id);
        }
    }
    heap.atoms.shrink_to_fit();
    heap.atom_next.shrink_to_fit();
}

impl Agent {
    /// Whether the heap has grown enough since the last collection that
    /// the caller should collect at its next safe point.
    pub fn should_collect(&self) -> bool {
        self.heap.bytes_used() >= self.heap.gc_threshold
    }

    /// Collect everything not reachable from the agent's intrinsics, the
    /// host root table and `roots`. Every handle the caller still uses
    /// must be reachable from one of them: a collected cell's index is
    /// reused, and reading a freed one panics.
    pub fn collect_garbage(&mut self, roots: Roots<'_>) -> GcStats {
        let heap = &self.heap;
        let mut marker = Marker {
            heap,
            marks: Marks {
                strings: BitSet::with_capacity(heap.strings.extent()),
                symbols: BitSet::with_capacity(heap.symbols.extent()),
                bigints: BitSet::with_capacity(heap.bigints.extent()),
                objects: BitSet::with_capacity(heap.objects.extent()),
                envs: BitSet::with_capacity(heap.envs.extent()),
                shapes: BitSet::with_capacity(heap.shapes.extent()),
            },
            work: Vec::new(),
            keys: Vec::new(),
        };
        marker.marks.shapes.insert(ROOT_SHAPE.0);
        for name in self.names.all() {
            marker.string(name);
        }
        for symbol in self.symbols.all() {
            marker.value(Value::symbol(symbol));
        }
        for object in self.intrinsics.all() {
            marker.value(Value::object(object));
        }
        for value in heap.roots.values() {
            marker.value(value);
        }
        for &value in roots.values {
            marker.value(value);
        }
        for &env in roots.envs {
            marker.env(env);
        }
        marker.drain();
        let marks = marker.marks;

        let heap = &mut self.heap;
        let bytes_before = heap.bytes_used();
        let stats = GcStats {
            bytes_before,
            bytes_after: 0,
            objects_freed: heap.objects.sweep(&marks.objects),
            strings_freed: heap.strings.sweep(&marks.strings),
            symbols_freed: heap.symbols.sweep(&marks.symbols),
            bigints_freed: heap.bigints.sweep(&marks.bigints),
            envs_freed: heap.envs.sweep(&marks.envs),
            shapes_freed: heap.shapes.sweep(&marks.shapes),
        };
        sweep_atoms(heap, &marks.strings);
        let live = live_footprint(heap);
        // Charges between collections are estimates of the same costs, so
        // the budget restarts from the exact live footprint.
        let used = heap.budget.used();
        if live <= used {
            let _ = heap.budget.release(used - live);
        } else {
            let _ = heap.budget.charge(live - used);
        }
        heap.gc_threshold = next_threshold(heap.bytes_used(), heap.byte_limit());
        heap.collections += 1;
        GcStats {
            bytes_after: heap.bytes_used(),
            ..stats
        }
    }

    /// Keep `value` alive across collections until [`Agent::unroot`].
    pub fn root(&mut self, value: Value) -> Result<HostRoot, OutOfMemory> {
        let heap = &mut self.heap;
        heap.charge(HOST_ROOT_BYTES)?;
        let roots = &mut heap.roots;
        let index = match roots.free.pop() {
            Some(index) => index,
            None => {
                let index = u32::try_from(roots.entries.len()).map_err(|_| OutOfMemory)?;
                roots.entries.push((0, None));
                index
            }
        };
        let entry = &mut roots.entries[index as usize];
        entry.1 = Some(value);
        roots.live += 1;
        Ok(HostRoot {
            index,
            generation: entry.0,
        })
    }

    /// The value behind a host root, or `None` once it was unrooted.
    pub fn rooted(&self, root: HostRoot) -> Option<Value> {
        match self.heap.roots.entries.get(root.index as usize) {
            Some(&(generation, value)) if generation == root.generation => value,
            _ => None,
        }
    }

    /// Release a host root. Returns `false` if it was already released.
    pub fn unroot(&mut self, root: HostRoot) -> bool {
        let heap = &mut self.heap;
        let Some(entry) = heap.roots.entries.get_mut(root.index as usize) else {
            return false;
        };
        if entry.0 != root.generation || entry.1.is_none() {
            return false;
        }
        entry.1 = None;
        // A slot whose generation would wrap is retired, never reused.
        if let Some(generation) = entry.0.checked_add(1) {
            entry.0 = generation;
            heap.roots.free.push(root.index);
        }
        heap.roots.live -= 1;
        let _ = heap.budget.release(HOST_ROOT_BYTES);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_sets_grow_and_report_fresh_bits() {
        let mut bits = BitSet::with_capacity(10);
        assert!(!bits.get(3));
        assert!(bits.insert(3));
        assert!(!bits.insert(3));
        assert!(bits.get(3));
        assert!(bits.insert(1000));
        assert!(bits.get(1000));
        assert!(!bits.get(999));
    }

    #[test]
    fn thresholds_scale_with_the_live_heap_and_respect_the_limit() {
        assert_eq!(next_threshold(0, 1 << 30), MIN_GC_THRESHOLD);
        assert_eq!(next_threshold(10 << 20, 1 << 30), 20 << 20);
        let limit = 16 << 20;
        assert_eq!(next_threshold(10 << 20, limit), limit / 8 * 7);
        assert_eq!(next_threshold(15 << 20, limit), 15 << 20);
        assert_eq!(initial_threshold(1000), 1000);
    }
}
