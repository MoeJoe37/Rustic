//! The heap: typed arenas indexed by `u32`, with every allocation charged
//! to a byte budget (ADR 0015). The mark/sweep collector over these arenas
//! is in `gc.rs` (JS-03); freed cells go on a free list so live indices
//! never move.

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::rc::Rc;

use rustic_base::Budget;

use super::bigint::BigInt;
use super::env::{EnvId, Environment};
use super::gc::{BitSet, HostRoots};
use super::object::{Closure, FLAG_PROTOTYPE, Object};
use super::shape::{PropertyKey, ShapeTable};
use super::string::JsStr;
use super::value::{BigIntId, ObjectId, StringId, SymbolId};
use crate::vm::bytecode::{CodeId, FunctionCode};

/// The heap ran out of its byte budget or of `u32` indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutOfMemory;

/// Default heap budget: 256 MiB.
pub const DEFAULT_HEAP_BYTES: usize = 256 * 1024 * 1024;

/// A symbol: its description, if any.
#[derive(Clone, Debug)]
pub struct Symbol {
    /// `[[Description]]`.
    pub description: Option<StringId>,
    /// A class's private name (`#x`): a property key that reflection
    /// never lists and that script never holds as a value.
    pub private: bool,
}

/// Cells of one type, indexed by `u32`. A freed cell is `None` and its
/// index waits on the free list; the lowest free index is reused first so
/// the arena stays compact and its tail can be returned.
#[derive(Debug)]
pub(crate) struct Arena<T> {
    cells: Vec<Option<T>>,
    /// Free indices, highest first, so `pop` reuses the lowest.
    free: Vec<u32>,
    live: usize,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self {
            cells: Vec::new(),
            free: Vec::new(),
            live: 0,
        }
    }
}

impl<T> Arena<T> {
    pub(crate) fn alloc(&mut self, value: T) -> Result<u32, OutOfMemory> {
        if let Some(index) = self.free.pop() {
            self.cells[index as usize] = Some(value);
            self.live += 1;
            return Ok(index);
        }
        let index = u32::try_from(self.cells.len()).map_err(|_| OutOfMemory)?;
        self.cells.push(Some(value));
        self.live += 1;
        Ok(index)
    }

    /// The live cell at `index`. Using a freed index is a bug in the
    /// caller's rooting and panics rather than aliasing another value.
    pub(crate) fn get(&self, index: u32) -> &T {
        self.cells[index as usize]
            .as_ref()
            .expect("use of a collected heap cell")
    }

    pub(crate) fn get_mut(&mut self, index: u32) -> &mut T {
        self.cells[index as usize]
            .as_mut()
            .expect("use of a collected heap cell")
    }

    pub(crate) fn live(&self) -> usize {
        self.live
    }

    /// Indices ever allocated (live or free).
    pub(crate) fn extent(&self) -> usize {
        self.cells.len()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (u32, &T)> {
        self.cells
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.as_ref().map(|c| (i as u32, c)))
    }

    /// Free every cell not in `marks`, drop the free tail and give its
    /// memory back. Returns the number of cells freed.
    pub(crate) fn sweep(&mut self, marks: &BitSet) -> usize {
        self.sweep_with(marks, |_| {})
    }

    /// [`Arena::sweep`], showing each cell to `on_free` before it goes.
    pub(crate) fn sweep_with(&mut self, marks: &BitSet, mut on_free: impl FnMut(&T)) -> usize {
        let mut freed = 0;
        for (i, cell) in self.cells.iter_mut().enumerate() {
            if !marks.get(i as u32) {
                if let Some(value) = cell.take() {
                    on_free(&value);
                    freed += 1;
                }
            }
        }
        self.live -= freed;
        while matches!(self.cells.last(), Some(None)) {
            self.cells.pop();
        }
        if self.cells.capacity() > 2 * self.cells.len() + 64 {
            self.cells.shrink_to_fit();
        }
        self.free.clear();
        self.free.extend(
            self.cells
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, c)| c.is_none())
                .map(|(i, _)| i as u32),
        );
        self.free.shrink_to_fit();
        freed
    }
}

/// One agent's heap.
#[derive(Debug)]
pub struct Heap {
    pub(crate) strings: Arena<JsStr>,
    /// Interned strings by content hash; collisions chain through
    /// `atom_next`.
    pub(crate) atoms: HashMap<u64, StringId>,
    pub(crate) atom_next: HashMap<StringId, StringId>,
    /// Which strings are atoms, so a string value that already is one
    /// becomes a property key without hashing its contents.
    pub(crate) interned: BitSet,
    hasher: RandomState,
    pub(crate) symbols: Arena<Symbol>,
    pub(crate) bigints: Arena<BigInt>,
    pub(crate) objects: Arena<Object>,
    pub(crate) shapes: ShapeTable,
    pub(crate) envs: Arena<Environment>,
    pub(crate) closures: Arena<Closure>,
    /// Compiled functions. The `Rc` lets a running frame read its code
    /// while the heap is borrowed mutably; liveness is still decided by
    /// the collector alone.
    pub(crate) codes: Arena<Rc<FunctionCode>>,
    pub(crate) roots: HostRoots,
    pub(crate) budget: Budget,
    /// Collect once `bytes_used` reaches this (see `gc.rs`).
    pub(crate) gc_threshold: usize,
    pub(crate) collections: u64,
    /// Ask for a collection at every safe point (GC stress testing).
    pub(crate) gc_stress: bool,
    /// Bumped whenever a named property of a prototype object changes or
    /// an object's prototype is replaced: what an inline cache learned
    /// by walking a prototype chain holds only within one value of this.
    pub(crate) proto_epoch: u32,
}

impl Heap {
    /// A heap that may hold `limit` bytes.
    pub fn new(limit: usize) -> Self {
        Self {
            strings: Arena::default(),
            atoms: HashMap::new(),
            atom_next: HashMap::new(),
            interned: BitSet::default(),
            hasher: RandomState::new(),
            symbols: Arena::default(),
            bigints: Arena::default(),
            objects: Arena::default(),
            shapes: ShapeTable::default(),
            envs: Arena::default(),
            closures: Arena::default(),
            codes: Arena::default(),
            roots: HostRoots::default(),
            budget: Budget::new(limit),
            gc_threshold: super::gc::initial_threshold(limit),
            collections: 0,
            gc_stress: false,
            proto_epoch: 0,
        }
    }

    /// Bytes charged so far.
    pub fn bytes_used(&self) -> usize {
        self.budget.used()
    }

    /// The byte limit.
    pub fn byte_limit(&self) -> usize {
        self.budget.limit()
    }

    /// Charge `bytes` against the budget.
    pub(crate) fn charge(&mut self, bytes: usize) -> Result<(), OutOfMemory> {
        self.budget.charge(bytes).map_err(|_| OutOfMemory)
    }

    // ---- strings ----------------------------------------------------

    /// Store a new string.
    pub fn alloc_string(&mut self, s: JsStr) -> Result<StringId, OutOfMemory> {
        self.charge(std::mem::size_of::<JsStr>() + s.storage_bytes())?;
        Ok(StringId(self.strings.alloc(s)?))
    }

    /// Store a string from Rust text.
    pub fn alloc_str(&mut self, text: &str) -> Result<StringId, OutOfMemory> {
        self.alloc_string(JsStr::from_text(text))
    }

    /// The contents of a string.
    pub fn string(&self, id: StringId) -> &JsStr {
        self.strings.get(id.0)
    }

    pub(crate) fn content_hash(&self, s: &JsStr) -> u64 {
        self.hasher.hash_one(s)
    }

    /// The interned copy of `s`: equal contents always give the same id, so
    /// property keys compare by id. Interned strings are shared, never
    /// mutated.
    pub fn intern(&mut self, s: &JsStr) -> Result<StringId, OutOfMemory> {
        let hash = self.content_hash(s);
        let mut current = self.atoms.get(&hash).copied();
        let mut last = None;
        while let Some(id) = current {
            if self.string(id) == s {
                return Ok(id);
            }
            last = Some(id);
            current = self.atom_next.get(&id).copied();
        }
        self.charge(ATOM_BYTES)?;
        let id = self.alloc_string(s.clone())?;
        self.interned.insert(id.0);
        match last {
            None => self.atoms.insert(hash, id),
            Some(last) => self.atom_next.insert(last, id),
        };
        Ok(id)
    }

    /// Intern Rust text.
    pub fn intern_str(&mut self, text: &str) -> Result<StringId, OutOfMemory> {
        self.intern(&JsStr::from_text(text))
    }

    /// The property key for a string in the heap: an array index or an
    /// atom. A string that is already an atom is not hashed again.
    pub fn key_for_string(&mut self, id: StringId) -> Result<PropertyKey, OutOfMemory> {
        if self.interned.get(id.0) {
            return Ok(match self.string(id).array_index() {
                Some(index) => PropertyKey::Index(index),
                None => PropertyKey::String(id),
            });
        }
        let contents = self.string(id).clone();
        self.key_for(&contents)
    }

    /// The property key for string contents: an array index or an atom.
    pub fn key_for(&mut self, s: &JsStr) -> Result<PropertyKey, OutOfMemory> {
        match s.array_index() {
            Some(index) => Ok(PropertyKey::Index(index)),
            None => Ok(PropertyKey::String(self.intern(s)?)),
        }
    }

    /// The property key for Rust text.
    pub fn key(&mut self, text: &str) -> Result<PropertyKey, OutOfMemory> {
        self.key_for(&JsStr::from_text(text))
    }

    // ---- symbols and BigInts ---------------------------------------

    /// A new unique symbol.
    pub fn alloc_symbol(&mut self, description: Option<StringId>) -> Result<SymbolId, OutOfMemory> {
        self.charge(std::mem::size_of::<Symbol>())?;
        Ok(SymbolId(self.symbols.alloc(Symbol {
            description,
            private: false,
        })?))
    }

    /// A fresh private name described by `description` (`#x`).
    pub(crate) fn alloc_private_name(
        &mut self,
        description: StringId,
    ) -> Result<SymbolId, OutOfMemory> {
        self.charge(std::mem::size_of::<Symbol>())?;
        Ok(SymbolId(self.symbols.alloc(Symbol {
            description: Some(description),
            private: true,
        })?))
    }

    /// A symbol's record.
    pub fn symbol(&self, id: SymbolId) -> &Symbol {
        self.symbols.get(id.0)
    }

    /// Store a BigInt.
    pub fn alloc_bigint(&mut self, b: BigInt) -> Result<BigIntId, OutOfMemory> {
        self.charge(std::mem::size_of::<BigInt>() + b.storage_bytes())?;
        Ok(BigIntId(self.bigints.alloc(b)?))
    }

    /// A BigInt's value.
    pub fn bigint(&self, id: BigIntId) -> &BigInt {
        self.bigints.get(id.0)
    }

    // ---- objects ----------------------------------------------------

    /// Store a new object.
    pub(crate) fn alloc_object(&mut self, object: Object) -> Result<ObjectId, OutOfMemory> {
        self.charge(std::mem::size_of::<Object>())?;
        if let Some(proto) = object.proto() {
            self.objects.get_mut(proto.0).set_flag(FLAG_PROTOTYPE, true);
        }
        Ok(ObjectId(self.objects.alloc(object)?))
    }

    /// An object.
    pub(crate) fn object(&self, id: ObjectId) -> &Object {
        self.objects.get(id.0)
    }

    /// An object, mutably.
    pub(crate) fn object_mut(&mut self, id: ObjectId) -> &mut Object {
        self.objects.get_mut(id.0)
    }

    /// Live objects.
    pub fn object_count(&self) -> usize {
        self.objects.live()
    }

    /// Live strings, atoms included.
    pub fn string_count(&self) -> usize {
        self.strings.live()
    }

    /// Live environment records.
    pub fn env_count(&self) -> usize {
        self.envs.live()
    }

    /// Store an environment record.
    pub(crate) fn alloc_env(&mut self, env: Environment) -> Result<EnvId, OutOfMemory> {
        self.charge(std::mem::size_of::<Environment>())?;
        Ok(EnvId(self.envs.alloc(env)?))
    }

    /// An environment record.
    pub(crate) fn env(&self, id: EnvId) -> &Environment {
        self.envs.get(id.0)
    }

    /// An environment record, mutably.
    pub(crate) fn env_mut(&mut self, id: EnvId) -> &mut Environment {
        self.envs.get_mut(id.0)
    }

    /// Note that a named property of `o` is about to change. If `o` is a
    /// prototype, inherited lookups other objects cached are stale.
    #[inline]
    pub(crate) fn note_named_change(&mut self, o: ObjectId) {
        if self.objects.get(o.0).has_flag(FLAG_PROTOTYPE) {
            self.proto_epoch = self.proto_epoch.wrapping_add(1);
        }
    }

    // ---- compiled code ------------------------------------------------

    /// Store a compiled function.
    pub(crate) fn alloc_code(&mut self, code: FunctionCode) -> Result<CodeId, OutOfMemory> {
        self.charge(code.code_bytes())?;
        Ok(CodeId(self.codes.alloc(Rc::new(code))?))
    }

    /// The closure of a function object compiled from source.
    pub(crate) fn closure(&self, f: ObjectId) -> Option<Closure> {
        self.object(f)
            .closure_index()
            .map(|index| *self.closures.get(index))
    }

    /// The closure of a function object, mutably.
    pub(crate) fn closure_mut(&mut self, f: ObjectId) -> Option<&mut Closure> {
        let index = self.object(f).closure_index()?;
        Some(self.closures.get_mut(index))
    }

    /// A compiled function.
    pub(crate) fn code(&self, id: CodeId) -> &Rc<FunctionCode> {
        self.codes.get(id.0)
    }

    /// Live compiled functions.
    pub fn code_count(&self) -> usize {
        self.codes.live()
    }

    /// Live shapes, the root included.
    pub fn shape_count(&self) -> usize {
        self.shapes.len()
    }

    /// Stress mode: [`Agent::should_collect`](super::Agent::should_collect)
    /// answers yes at every safe point, so a collection runs as often as the
    /// embedder can run one. Tests use it to find handles that are not
    /// rooted.
    pub fn set_gc_stress(&mut self, on: bool) {
        self.gc_stress = on;
    }

    /// Completed collections.
    pub fn collections(&self) -> u64 {
        self.collections
    }
}

/// Budget charge for an atom's table entries, on top of its string.
pub(crate) const ATOM_BYTES: usize = 24;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_deduplicates_across_representations() {
        let mut heap = Heap::new(DEFAULT_HEAP_BYTES);
        let a = heap.intern_str("width").unwrap();
        let b = heap
            .intern(&JsStr::from_units(
                &"width".encode_utf16().collect::<Vec<_>>(),
            ))
            .unwrap();
        assert_eq!(a, b);
        assert_ne!(a, heap.intern_str("height").unwrap());
        let wide = heap.intern_str("€").unwrap();
        assert_eq!(wide, heap.intern_str("€").unwrap());
        assert_eq!(heap.key("7").unwrap(), PropertyKey::Index(7));
        assert_eq!(
            heap.key("07").unwrap(),
            PropertyKey::String(heap.intern_str("07").unwrap())
        );
    }

    #[test]
    fn allocations_are_charged_and_bounded() {
        let mut heap = Heap::new(200);
        let before = heap.bytes_used();
        heap.alloc_str("abcd").unwrap();
        let header = std::mem::size_of::<JsStr>();
        assert_eq!(heap.bytes_used() - before, header + 4);
        assert_eq!(heap.alloc_str(&"x".repeat(500)), Err(OutOfMemory));
        // A failed allocation charges nothing.
        assert_eq!(heap.bytes_used() - before, header + 4);
    }
}
