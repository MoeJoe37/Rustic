//! Shapes (hidden classes): a shared transition tree describing which
//! named properties an object has, with what attributes and in which slot
//! (ADR 0015). Objects built the same way share one chain, so each object
//! stores only its slot values.

use super::gc::BitSet;
use super::value::{StringId, SymbolId};

/// A property key (§6.1.7): an interned string, a symbol, or an array
/// index, which is never stored as a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PropertyKey {
    /// A canonical array index, 0 to 2³² − 2.
    Index(u32),
    /// An interned (atom) string that is not an array index.
    String(StringId),
    /// A symbol.
    Symbol(SymbolId),
}

/// Property attributes as bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Attributes(u8);

impl Attributes {
    /// `[[Writable]]` (data properties only).
    pub const WRITABLE: u8 = 1;
    /// `[[Enumerable]]`.
    pub const ENUMERABLE: u8 = 2;
    /// `[[Configurable]]`.
    pub const CONFIGURABLE: u8 = 4;
    /// The property is an accessor (two slots: getter, setter).
    pub const ACCESSOR: u8 = 8;

    /// Writable, enumerable and configurable data: what assignment creates.
    pub const DEFAULT: Attributes =
        Attributes(Self::WRITABLE | Self::ENUMERABLE | Self::CONFIGURABLE);

    /// Attributes from bits.
    pub const fn new(bits: u8) -> Self {
        Attributes(bits & 0xF)
    }

    /// The bits.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Whether `bit` is set.
    pub const fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }

    /// Whether this is an accessor property.
    pub const fn is_accessor(self) -> bool {
        self.has(Self::ACCESSOR)
    }

    /// Slots the property occupies.
    pub const fn slots(self) -> u32 {
        if self.is_accessor() { 2 } else { 1 }
    }
}

/// A shape in a [`ShapeTable`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShapeId(pub(crate) u32);

/// The root shape every new object starts from.
pub const ROOT_SHAPE: ShapeId = ShapeId(0);

/// Objects with more named properties than this leave the shared tree for
/// a dictionary of their own, bounding lookup walks and tree growth.
pub const MAX_SHAPE_PROPERTIES: u32 = 32;

#[derive(Clone, Debug)]
struct Shape {
    parent: Option<ShapeId>,
    /// The property this shape adds (meaningless for the root).
    key: PropertyKey,
    attributes: Attributes,
    /// First slot of that property.
    slot: u32,
    /// Slots used by the whole chain.
    slot_count: u32,
    /// Properties in the whole chain.
    property_count: u32,
    /// Shapes reached by adding one property.
    transitions: Vec<(PropertyKey, Attributes, ShapeId)>,
}

/// A property found in a shape chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    /// First slot index.
    pub slot: u32,
    /// The property's attributes.
    pub attributes: Attributes,
}

/// Every shape of one heap. Shapes no live object uses are freed by the
/// collector, and their indices reused.
#[derive(Debug)]
pub struct ShapeTable {
    shapes: Vec<Option<Shape>>,
    /// Free indices, highest first.
    free: Vec<u32>,
    live: usize,
}

impl Default for ShapeTable {
    fn default() -> Self {
        Self {
            shapes: vec![Some(Shape {
                parent: None,
                key: PropertyKey::Index(0),
                attributes: Attributes::default(),
                slot: 0,
                slot_count: 0,
                property_count: 0,
                transitions: Vec::new(),
            })],
            free: Vec::new(),
            live: 1,
        }
    }
}

impl ShapeTable {
    /// Live shapes, the root included.
    pub fn len(&self) -> usize {
        self.live
    }

    fn get(&self, id: ShapeId) -> &Shape {
        self.shapes[id.0 as usize]
            .as_ref()
            .expect("use of a collected shape")
    }

    /// Find `key` in the chain ending at `id`.
    pub fn lookup(&self, id: ShapeId, key: PropertyKey) -> Option<Slot> {
        let mut current = Some(id);
        while let Some(shape_id) = current {
            let shape = self.get(shape_id);
            let parent = shape.parent?;
            if shape.key == key {
                return Some(Slot {
                    slot: shape.slot,
                    attributes: shape.attributes,
                });
            }
            current = Some(parent);
        }
        None
    }

    /// The shape after adding `key` with `attributes` to `id`, shared with
    /// every object that made the same transition. Returns `None` when the
    /// object should become a dictionary instead.
    pub fn add(
        &mut self,
        id: ShapeId,
        key: PropertyKey,
        attributes: Attributes,
    ) -> Option<ShapeId> {
        let shape = self.get(id);
        if shape.property_count >= MAX_SHAPE_PROPERTIES {
            return None;
        }
        if let Some(&(_, _, next)) = shape
            .transitions
            .iter()
            .find(|(k, a, _)| *k == key && *a == attributes)
        {
            return Some(next);
        }
        let slot = shape.slot_count;
        let new = Shape {
            parent: Some(id),
            key,
            attributes,
            slot,
            slot_count: slot + attributes.slots(),
            property_count: shape.property_count + 1,
            transitions: Vec::new(),
        };
        let next = match self.free.pop() {
            Some(index) => {
                self.shapes[index as usize] = Some(new);
                ShapeId(index)
            }
            None => {
                let next = ShapeId(u32::try_from(self.shapes.len()).ok()?);
                self.shapes.push(Some(new));
                next
            }
        };
        self.live += 1;
        self.shapes[id.0 as usize]
            .as_mut()
            .expect("parent shape is live")
            .transitions
            .push((key, attributes, next));
        Some(next)
    }

    /// The parent of `id` if `key` is the last property it added (the
    /// common delete-the-newest case), so the object can keep sharing.
    pub fn remove_last(&self, id: ShapeId, key: PropertyKey) -> Option<ShapeId> {
        let shape = self.get(id);
        if shape.key == key { shape.parent } else { None }
    }

    /// Slots used by objects of shape `id`.
    pub fn slot_count(&self, id: ShapeId) -> u32 {
        self.get(id).slot_count
    }

    /// Whether any property of the chain satisfies `test`, without
    /// building the property list.
    pub fn any(&self, id: ShapeId, mut test: impl FnMut(PropertyKey, Attributes) -> bool) -> bool {
        let mut current = id;
        while let Some(parent) = self.get(current).parent {
            let shape = self.get(current);
            if test(shape.key, shape.attributes) {
                return true;
            }
            current = parent;
        }
        false
    }

    /// Every property of the chain in insertion order.
    pub fn properties(&self, id: ShapeId) -> Vec<(PropertyKey, Slot)> {
        let mut out = Vec::with_capacity(self.get(id).property_count as usize);
        let mut current = id;
        while let Some(parent) = self.get(current).parent {
            let shape = self.get(current);
            out.push((
                shape.key,
                Slot {
                    slot: shape.slot,
                    attributes: shape.attributes,
                },
            ));
            current = parent;
        }
        out.reverse();
        out
    }

    /// Indices ever allocated.
    pub(crate) fn extent(&self) -> usize {
        self.shapes.len()
    }

    /// Mark `id` and its ancestors, reporting each newly marked shape's
    /// key. Stops at the first shape already marked, so a whole collection
    /// visits each shape once.
    pub(crate) fn mark_chain(
        &self,
        id: ShapeId,
        marks: &mut BitSet,
        mut key: impl FnMut(PropertyKey),
    ) {
        let mut current = Some(id);
        while let Some(shape_id) = current {
            if !marks.insert(shape_id.0) {
                return;
            }
            let shape = self.get(shape_id);
            if shape.parent.is_some() {
                key(shape.key);
            }
            current = shape.parent;
        }
    }

    /// Free every shape outside `marks` (the root always stays) and drop
    /// transitions to freed shapes. Returns the number freed.
    pub(crate) fn sweep(&mut self, marks: &BitSet) -> usize {
        let mut freed = 0;
        for (i, cell) in self.shapes.iter_mut().enumerate().skip(1) {
            if cell.is_some() && !marks.get(i as u32) {
                *cell = None;
                freed += 1;
            }
        }
        self.live -= freed;
        if freed > 0 {
            for shape in self.shapes.iter_mut().flatten() {
                shape.transitions.retain(|(_, _, next)| marks.get(next.0));
            }
        }
        while matches!(self.shapes.last(), Some(None)) {
            self.shapes.pop();
        }
        if self.shapes.capacity() > 2 * self.shapes.len() + 64 {
            self.shapes.shrink_to_fit();
        }
        self.free.clear();
        self.free.extend(
            self.shapes
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(i: u32) -> PropertyKey {
        PropertyKey::String(StringId(i))
    }

    #[test]
    fn identical_construction_shares_shapes() {
        let mut table = ShapeTable::default();
        let a1 = table.add(ROOT_SHAPE, key(1), Attributes::DEFAULT).unwrap();
        let ab1 = table.add(a1, key(2), Attributes::DEFAULT).unwrap();
        let a2 = table.add(ROOT_SHAPE, key(1), Attributes::DEFAULT).unwrap();
        let ab2 = table.add(a2, key(2), Attributes::DEFAULT).unwrap();
        assert_eq!((a1, ab1), (a2, ab2));
        assert_eq!(table.len(), 3);
        // Different order or attributes make a different shape.
        let b = table.add(ROOT_SHAPE, key(2), Attributes::DEFAULT).unwrap();
        assert_ne!(b, a1);
        let a_ro = table
            .add(ROOT_SHAPE, key(1), Attributes::new(Attributes::ENUMERABLE))
            .unwrap();
        assert_ne!(a_ro, a1);
        assert_eq!(table.lookup(ab1, key(2)).unwrap().slot, 1);
        assert_eq!(table.lookup(ab1, key(3)), None);
        assert_eq!(
            table
                .properties(ab1)
                .iter()
                .map(|p| p.0)
                .collect::<Vec<_>>(),
            [key(1), key(2)]
        );
        assert_eq!(table.remove_last(ab1, key(2)), Some(a1));
        assert_eq!(table.remove_last(ab1, key(1)), None);
    }

    #[test]
    fn accessors_take_two_slots_and_chains_are_bounded() {
        let mut table = ShapeTable::default();
        let get = table
            .add(ROOT_SHAPE, key(1), Attributes::new(Attributes::ACCESSOR))
            .unwrap();
        let next = table.add(get, key(2), Attributes::DEFAULT).unwrap();
        assert_eq!(table.lookup(next, key(2)).unwrap().slot, 2);
        assert_eq!(table.slot_count(next), 3);
        let mut shape = ROOT_SHAPE;
        for i in 0..MAX_SHAPE_PROPERTIES {
            shape = table.add(shape, key(100 + i), Attributes::DEFAULT).unwrap();
        }
        assert_eq!(table.add(shape, key(999), Attributes::DEFAULT), None);
    }
}
