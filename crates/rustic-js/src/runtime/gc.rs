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
use super::heap::{ATOM_BYTES, Heap, OutOfMemory, ROPE_BYTES, Symbol};
use super::object::{Closure, Internal, Object};
use super::shape::{PropertyKey, ROOT_SHAPE};
use super::string::JsStr;
use super::value::{Kind, StringId, Value};
use crate::runtime::bigint::BigInt;
use crate::vm::bytecode::CodeId;

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
    fn code(&mut self, id: CodeId);

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
    /// Compiled functions the caller holds (code about to run).
    pub codes: &'a [CodeId],
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
    /// Compiled functions freed.
    pub codes_freed: usize,
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
    codes: BitSet,
    closures: BitSet,
}

impl Marks {
    /// Whether a weakly held value survives: primitives always do.
    fn is_live(&self, value: Value) -> bool {
        match value.kind() {
            Kind::Object(id) => self.objects.get(id.0),
            Kind::Symbol(id) => self.symbols.get(id.0),
            _ => true,
        }
    }
}

/// What the embedder contributes to a collection: the edges of its host
/// objects. The JS heap knows a host object only by the `u64` it was
/// created with (ADR 0016); a DOM binding uses it to name a node.
pub trait HostTracer {
    /// Report values the host keeps alive on its own (wrappers and
    /// listeners of nodes in a document, for example).
    fn roots(&mut self, _sink: &mut dyn FnMut(Value)) {}

    /// A host object created with `data` is reachable: report every value
    /// it keeps alive. Called once per reachable host object.
    fn trace(&mut self, data: u64, sink: &mut dyn FnMut(Value));

    /// A host object created with `data` was collected.
    fn finalize(&mut self, _data: u64) {}
}

/// A host with no host objects.
struct NoHost;

impl HostTracer for NoHost {
    fn trace(&mut self, _: u64, _: &mut dyn FnMut(Value)) {}
}

enum Gray {
    Object(u32),
    Env(u32),
    Code(u32),
}

struct Marker<'h> {
    heap: &'h Heap,
    marks: Marks,
    work: Vec<Gray>,
    /// Scratch for keys of newly marked shapes.
    keys: Vec<PropertyKey>,
    /// Reachable host objects whose edges the host has not reported yet.
    hosts: Vec<u64>,
    /// Reachable weak objects, fixed up after marking.
    weak_refs: Vec<u32>,
    weak_maps: Vec<u32>,
    registries: Vec<u32>,
    /// Map and Set objects, and the tables live iterators walk, for
    /// recounting open iterators.
    tables: Vec<u32>,
    table_iterators: Vec<u32>,
    /// A symbol or object was newly marked since the flag was cleared, so
    /// another ephemeron pass may find more live keys.
    progress: bool,
}

impl Tracer for Marker<'_> {
    fn value(&mut self, value: Value) {
        match value.kind() {
            Kind::String(id) => self.string(id),
            Kind::Symbol(id) => {
                if self.marks.symbols.insert(id.0) {
                    self.progress = true;
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
                    self.progress = true;
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
        if !self.marks.strings.insert(id.0) {
            return;
        }
        // A rope keeps its halves alive until it is flattened. Chains of
        // appends are deep, so walk them with a list, not recursion.
        let Some((left, right)) = self.heap.rope_halves(id) else {
            return;
        };
        // A rope reached from outside a rope is one the program still
        // holds. Its chain usually costs far more than its text, so copy
        // the text now and let the sweep free the chain; unless the copy
        // would not fit (a string doubled onto itself is a small rope).
        if self.heap.rope_fits(id) {
            let _ = self.heap.string(id);
            return;
        }
        let mut pending = vec![left, right];
        while let Some(part) = pending.pop() {
            if self.marks.strings.insert(part.0) {
                if let Some((left, right)) = self.heap.rope_halves(part) {
                    pending.push(left);
                    pending.push(right);
                }
            }
        }
    }

    fn env(&mut self, id: EnvId) {
        if self.marks.envs.insert(id.0) {
            self.work.push(Gray::Env(id.0));
        }
    }

    fn code(&mut self, id: CodeId) {
        if self.marks.codes.insert(id.0) {
            self.work.push(Gray::Code(id.0));
        }
    }
}

impl Marker<'_> {
    /// Mark everything reachable from the worklist, asking `host` for the
    /// edges of host objects and treating weak-map entries as ephemerons:
    /// a value is marked only once its key is. Repeats until nothing new
    /// is found.
    fn drain(&mut self, host: &mut dyn HostTracer) {
        loop {
            while let Some(gray) = self.work.pop() {
                let heap = self.heap;
                match gray {
                    Gray::Object(index) => self.trace_object(index, heap.objects.get(index)),
                    Gray::Env(index) => heap.envs.get(index).trace(self),
                    Gray::Code(index) => heap.codes.get(index).trace(self),
                }
            }
            while let Some(data) = self.hosts.pop() {
                host.trace(data, &mut |value| self.value(value));
            }
            if !self.work.is_empty() {
                continue;
            }
            self.progress = false;
            let heap = self.heap;
            for at in 0..self.weak_maps.len() {
                let map = heap.objects.get(self.weak_maps[at]);
                if let Some(Internal::WeakMap(entries)) = map.internal() {
                    for (&key, &value) in entries {
                        if self.marks.is_live(Value::from_raw(key)) {
                            self.value(value);
                        }
                    }
                }
            }
            if !self.progress {
                return;
            }
        }
    }

    fn trace_object(&mut self, index: u32, object: &Object) {
        if let Some(closure) = object.closure_index() {
            self.marks.closures.insert(closure);
            let closure = *self.heap.closures.get(closure);
            self.code(closure.code);
            self.env(closure.env);
            self.value(closure.this);
            self.value(closure.home);
        }
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
            match &extra.internal {
                None | Some(Internal::ImportRequest(_)) => {}
                Some(Internal::Host(data)) => self.hosts.push(*data),
                Some(Internal::Iterator { target, kind, .. }) => {
                    self.value(*target);
                    if kind.walks_table() {
                        if let Some(table) = target.as_object() {
                            self.table_iterators.push(table.0);
                        }
                    }
                }
                Some(Internal::Table(table)) => {
                    for v in table.values() {
                        self.value(v);
                    }
                    self.tables.push(index);
                }
                Some(Internal::Helper(helper)) => {
                    for v in helper.values() {
                        self.value(v);
                    }
                }
                Some(
                    Internal::Wrapped { iterator, next }
                    | Internal::AsyncFromSync { iterator, next }
                    | Internal::RegExp {
                        source: iterator,
                        flags: next,
                        ..
                    }
                    | Internal::RegExpStringIterator {
                        regexp: iterator,
                        string: next,
                        ..
                    },
                ) => {
                    self.value(*iterator);
                    self.value(*next);
                }
                Some(Internal::Generator(generator)) => {
                    for v in generator.values() {
                        self.value(v);
                    }
                    self.code(generator.code);
                    self.env(generator.env);
                }
                Some(Internal::Promise(promise)) => {
                    for v in promise.values() {
                        self.value(v);
                    }
                }
                Some(Internal::ListFormat { .. }) => {}
                Some(Internal::Locale(tag)) => self.value(Value::string(*tag)),
                Some(Internal::ModuleEvaluation(graph)) => graph.trace(self),
                Some(Internal::Captures(values)) => {
                    for &v in values.iter() {
                        self.value(v);
                    }
                }
                Some(Internal::Proxy { target, handler }) => {
                    for o in [target, handler].into_iter().flatten() {
                        self.value(Value::object(*o));
                    }
                }
                Some(Internal::Arguments { env, .. }) => self.env(*env),
                Some(Internal::ModuleNamespace(exports, env)) => {
                    self.env(*env);
                    for &(key, binding) in exports.iter() {
                        if let PropertyKey::String(name) = key {
                            self.string(name);
                        }
                        binding.trace(self);
                    }
                }
                Some(Internal::TypedArray { buffer, .. } | Internal::DataView { buffer, .. }) => {
                    self.value(Value::object(*buffer));
                }
                Some(Internal::Bound { target, this, args }) => {
                    self.value(Value::object(*target));
                    self.value(*this);
                    for &v in args.iter() {
                        self.value(v);
                    }
                }
                // The target is weak: cleared after marking if nothing
                // else reaches it.
                // Bytes hold no references.
                Some(Internal::ArrayBuffer(_)) => {}
                Some(Internal::WeakRef(_)) => self.weak_refs.push(index),
                Some(Internal::WeakMap(_)) => self.weak_maps.push(index),
                Some(Internal::Registry { cells, pending }) => {
                    for cell in cells {
                        self.value(cell.held);
                    }
                    for &held in pending {
                        self.value(held);
                    }
                    self.registries.push(index);
                }
            }
        }
    }
}

/// Budget charge per weak-map entry or registry cell.
pub(crate) const WEAK_ENTRY_BYTES: usize = 24;

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
        bytes += match &extra.internal {
            Some(Internal::WeakMap(entries)) => WEAK_ENTRY_BYTES * entries.len(),
            Some(Internal::Bound { args, .. }) => 8 * args.len(),
            Some(Internal::Arguments { slots, .. }) => 4 * slots.len(),
            Some(Internal::ModuleNamespace(exports, _)) => std::mem::size_of_val(exports.as_ref()),
            Some(Internal::ArrayBuffer(data)) => data.bytes.as_ref().map_or(0, Vec::len),
            Some(Internal::Table(table)) => table.footprint(),
            Some(Internal::Helper(helper)) => {
                std::mem::size_of::<super::object::Helper>() + 8 * helper.items.len()
            }
            Some(Internal::Generator(generator)) => {
                std::mem::size_of::<super::object::Generator>()
                    + 8 * generator.registers.len()
                    + std::mem::size_of::<super::object::AsyncRequest>() * generator.queue.len()
            }
            Some(Internal::Promise(promise)) => {
                std::mem::size_of::<super::object::PromiseRecord>()
                    + std::mem::size_of::<super::object::Reaction>()
                        * (promise.fulfill_reactions.len() + promise.reject_reactions.len())
            }
            Some(Internal::ModuleEvaluation(graph)) => graph.footprint(),
            Some(Internal::ImportRequest(request)) => request.footprint(),
            Some(Internal::Captures(values)) => 8 * values.len(),
            Some(Internal::RegExp { matcher, .. }) => matcher.footprint(),
            Some(Internal::Registry { cells, pending }) => {
                WEAK_ENTRY_BYTES * cells.len() + 8 * pending.len()
            }
            _ => 0,
        };
    }
    bytes
}

/// Bytes the live heap is charged for, computed from scratch.
fn live_footprint(heap: &Heap) -> usize {
    let strings: usize = heap
        .strings
        .iter()
        .map(|(_, s)| {
            std::mem::size_of::<JsStr>() + s.get().map_or(ROPE_BYTES, JsStr::storage_bytes)
        })
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
    let codes: usize = heap.codes.iter().map(|(_, c)| c.code_bytes()).sum();
    let closures = heap.closures.live() * std::mem::size_of::<Closure>();
    strings + atoms + symbols + bigints + objects + envs + shapes + roots + codes + closures
}

/// Rebuild the atom table without freed strings. Chains keep their order,
/// so lookups behave as before.
fn sweep_atoms(heap: &mut Heap, marks: &BitSet) {
    let heads = std::mem::take(&mut heap.atoms);
    let mut next = std::mem::take(&mut heap.atom_next);
    let mut interned = BitSet::with_capacity(heap.strings.extent());
    for (hash, head) in heads {
        let mut last: Option<StringId> = None;
        let mut current = Some(head);
        while let Some(id) = current {
            current = next.remove(&id);
            if !marks.get(id.0) {
                continue;
            }
            interned.insert(id.0);
            match last {
                None => heap.atoms.insert(hash, id),
                Some(prev) => heap.atom_next.insert(prev, id),
            };
            last = Some(id);
        }
    }
    heap.interned = interned;
    heap.atoms.shrink_to_fit();
    heap.atom_next.shrink_to_fit();
}

impl Agent {
    /// Whether the heap has grown enough since the last collection that
    /// the caller should collect at its next safe point.
    pub fn should_collect(&self) -> bool {
        self.heap.gc_stress || self.heap.bytes_used() >= self.heap.gc_threshold
    }

    /// Collect everything not reachable from the agent's intrinsics, the
    /// host root table and `roots`. Every handle the caller still uses
    /// must be reachable from one of them: a collected cell's index is
    /// reused, and reading a freed one panics.
    pub fn collect_garbage(&mut self, roots: Roots<'_>) -> GcStats {
        self.collect_garbage_with(roots, &mut NoHost)
    }

    /// [`Agent::collect_garbage`] for an embedder with host objects:
    /// `host` adds its own roots, reports the edges of each reachable host
    /// object, and learns which host objects were collected.
    pub fn collect_garbage_with(&mut self, roots: Roots<'_>, host: &mut dyn HostTracer) -> GcStats {
        let started = self.heap.gc_clock.map(|clock| clock());
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
                codes: BitSet::with_capacity(heap.codes.extent()),
                closures: BitSet::with_capacity(heap.closures.extent()),
            },
            work: Vec::new(),
            keys: Vec::new(),
            hosts: Vec::new(),
            weak_refs: Vec::new(),
            weak_maps: Vec::new(),
            registries: Vec::new(),
            tables: Vec::new(),
            table_iterators: Vec::new(),
            progress: false,
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
        for &code in roots.codes {
            marker.code(code);
        }
        marker.env(self.global_env);
        self.vm.trace(&mut marker);
        host.roots(&mut |value| marker.value(value));
        marker.drain(host);
        let Marker {
            marks,
            weak_refs,
            weak_maps,
            registries,
            tables,
            table_iterators,
            ..
        } = marker;

        let heap = &mut self.heap;
        let bytes_before = heap.bytes_used();
        // Weak fix-ups, while every cell is still readable. Marking is
        // complete, so held values moved to `pending` are already marked.
        for index in weak_refs {
            if let Some(Internal::WeakRef(target)) = heap.objects.get_mut(index).internal_mut() {
                if !marks.is_live(*target) {
                    *target = Value::UNINITIALIZED;
                }
            }
        }
        // Only iterators that survive hold a table's positions.
        for &index in &tables {
            if let Some(Internal::Table(table)) = heap.objects.get_mut(index).internal_mut() {
                table.iterators = 0;
            }
        }
        for index in table_iterators {
            if let Some(Internal::Table(table)) = heap.objects.get_mut(index).internal_mut() {
                table.iterators += 1;
            }
        }
        for index in weak_maps {
            if let Some(Internal::WeakMap(entries)) = heap.objects.get_mut(index).internal_mut() {
                entries.retain(|&key, _| marks.is_live(Value::from_raw(key)));
                if entries.capacity() > 2 * entries.len() + 8 {
                    entries.shrink_to_fit();
                }
            }
        }
        for index in registries {
            if let Some(Internal::Registry { cells, pending }) =
                heap.objects.get_mut(index).internal_mut()
            {
                cells.retain_mut(|cell| {
                    if !marks.is_live(cell.target) {
                        pending.push(cell.held);
                        return false;
                    }
                    if !marks.is_live(cell.token) {
                        cell.token = Value::UNINITIALIZED;
                    }
                    true
                });
            }
        }
        let mut finalized = Vec::new();
        let objects_freed = heap.objects.sweep_with(&marks.objects, |object| {
            if let Some(Internal::Host(data)) = object.internal() {
                finalized.push(*data);
            }
        });
        let stats = GcStats {
            bytes_before,
            bytes_after: 0,
            objects_freed,
            strings_freed: heap.strings.sweep(&marks.strings),
            symbols_freed: heap.symbols.sweep(&marks.symbols),
            bigints_freed: heap.bigints.sweep(&marks.bigints),
            envs_freed: heap.envs.sweep(&marks.envs),
            shapes_freed: heap.shapes.sweep(&marks.shapes),
            codes_freed: heap.codes.sweep(&marks.codes),
        };
        heap.closures.sweep(&marks.closures);
        sweep_atoms(heap, &marks.strings);
        // Ropes that died or were flattened no longer need their halves.
        let strings = &heap.strings;
        heap.ropes.retain(|&index, _| {
            strings
                .try_get(index)
                .is_some_and(|cell| cell.get().is_none())
        });
        if heap.ropes.capacity() > 2 * heap.ropes.len() + 64 {
            heap.ropes.shrink_to_fit();
        }
        heap.settle_after_collection();
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
        if let (Some(clock), Some(started)) = (heap.gc_clock, started) {
            let pause = clock().saturating_sub(started);
            heap.gc_pause_nanos = heap.gc_pause_nanos.saturating_add(pause);
            heap.gc_pause_max_nanos = heap.gc_pause_max_nanos.max(pause);
        }
        for data in finalized {
            host.finalize(data);
        }
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
