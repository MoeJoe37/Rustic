//! The heap: typed arenas indexed by `u32`, with every allocation charged
//! to a byte budget (ADR 0015). Nothing is collected yet; mark/sweep over
//! these arenas arrives with JS-03 (TD-91).

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};

use rustic_base::Budget;

use super::bigint::BigInt;
use super::env::{EnvId, Environment};
use super::object::Object;
use super::shape::{PropertyKey, ShapeTable};
use super::string::JsStr;
use super::value::{BigIntId, ObjectId, StringId, SymbolId};

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
}

/// One agent's heap.
#[derive(Debug)]
pub struct Heap {
    strings: Vec<JsStr>,
    /// Interned strings by content hash; collisions chain through
    /// `atom_next`.
    atoms: HashMap<u64, StringId>,
    atom_next: HashMap<StringId, StringId>,
    hasher: RandomState,
    symbols: Vec<Symbol>,
    bigints: Vec<BigInt>,
    pub(crate) objects: Vec<Object>,
    pub(crate) shapes: ShapeTable,
    envs: Vec<Environment>,
    budget: Budget,
}

impl Heap {
    /// A heap that may hold `limit` bytes.
    pub fn new(limit: usize) -> Self {
        Self {
            strings: Vec::new(),
            atoms: HashMap::new(),
            atom_next: HashMap::new(),
            hasher: RandomState::new(),
            symbols: Vec::new(),
            bigints: Vec::new(),
            objects: Vec::new(),
            shapes: ShapeTable::default(),
            envs: Vec::new(),
            budget: Budget::new(limit),
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

    fn next_index(len: usize) -> Result<u32, OutOfMemory> {
        u32::try_from(len).map_err(|_| OutOfMemory)
    }

    // ---- strings ----------------------------------------------------

    /// Store a new string.
    pub fn alloc_string(&mut self, s: JsStr) -> Result<StringId, OutOfMemory> {
        let id = StringId(Self::next_index(self.strings.len())?);
        self.charge(std::mem::size_of::<JsStr>() + s.storage_bytes())?;
        self.strings.push(s);
        Ok(id)
    }

    /// Store a string from Rust text.
    pub fn alloc_str(&mut self, text: &str) -> Result<StringId, OutOfMemory> {
        self.alloc_string(JsStr::from_text(text))
    }

    /// The contents of a string.
    pub fn string(&self, id: StringId) -> &JsStr {
        &self.strings[id.0 as usize]
    }

    fn content_hash(&self, s: &JsStr) -> u64 {
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
        self.charge(24)?;
        let id = self.alloc_string(s.clone())?;
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
        let id = SymbolId(Self::next_index(self.symbols.len())?);
        self.charge(std::mem::size_of::<Symbol>())?;
        self.symbols.push(Symbol { description });
        Ok(id)
    }

    /// A symbol's record.
    pub fn symbol(&self, id: SymbolId) -> &Symbol {
        &self.symbols[id.0 as usize]
    }

    /// Store a BigInt.
    pub fn alloc_bigint(&mut self, b: BigInt) -> Result<BigIntId, OutOfMemory> {
        let id = BigIntId(Self::next_index(self.bigints.len())?);
        self.charge(std::mem::size_of::<BigInt>() + b.storage_bytes())?;
        self.bigints.push(b);
        Ok(id)
    }

    /// A BigInt's value.
    pub fn bigint(&self, id: BigIntId) -> &BigInt {
        &self.bigints[id.0 as usize]
    }

    // ---- objects ----------------------------------------------------

    /// Store a new object.
    pub(crate) fn alloc_object(&mut self, object: Object) -> Result<ObjectId, OutOfMemory> {
        let id = ObjectId(Self::next_index(self.objects.len())?);
        self.charge(std::mem::size_of::<Object>())?;
        self.objects.push(object);
        Ok(id)
    }

    /// An object.
    pub(crate) fn object(&self, id: ObjectId) -> &Object {
        &self.objects[id.0 as usize]
    }

    /// An object, mutably.
    pub(crate) fn object_mut(&mut self, id: ObjectId) -> &mut Object {
        &mut self.objects[id.0 as usize]
    }

    /// Objects allocated so far.
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    /// Store an environment record.
    pub(crate) fn alloc_env(&mut self, env: Environment) -> Result<EnvId, OutOfMemory> {
        let id = EnvId(Self::next_index(self.envs.len())?);
        self.charge(std::mem::size_of::<Environment>())?;
        self.envs.push(env);
        Ok(id)
    }

    /// An environment record.
    pub(crate) fn env(&self, id: EnvId) -> &Environment {
        &self.envs[id.0 as usize]
    }

    /// An environment record, mutably.
    pub(crate) fn env_mut(&mut self, id: EnvId) -> &mut Environment {
        &mut self.envs[id.0 as usize]
    }

    /// Shapes created so far.
    pub fn shape_count(&self) -> usize {
        self.shapes.len()
    }
}

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
