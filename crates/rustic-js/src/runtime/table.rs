//! The ordered hash table behind `Map` and `Set` (§24.1, §24.2).
//!
//! Entries stay in insertion order in one vector. A deleted entry leaves
//! a hole (its key becomes `UNINITIALIZED`) so that iterators, which hold
//! a position, keep their place. Holes are squeezed out when they
//! outnumber the live entries and no iterator is open; the collector
//! recounts the open iterators of every table, so an iterator abandoned
//! half-way stops holding a table back after the next collection.
//!
//! Keys compare with SameValueZero. Each entry costs 24 bytes plus its
//! share of the bucket index.

use std::collections::HashMap;

use super::agent::Agent;
use super::value::{Kind, Value};

const NONE: u32 = u32::MAX;

#[derive(Clone, Copy, Debug)]
struct Entry {
    key: Value,
    value: Value,
    /// The next entry in the same bucket.
    next: u32,
}

/// An insertion-ordered table of SameValueZero keys.
#[derive(Clone, Debug, Default)]
pub(crate) struct OrderedTable {
    entries: Vec<Entry>,
    /// The first entry of each hash's bucket.
    buckets: HashMap<u64, u32>,
    live: u32,
    /// Iterator objects that hold a position; the collector recounts them.
    pub(crate) iterators: u32,
    /// Native loops (`forEach`) that hold a position. They are not heap
    /// objects, so the collector leaves this count alone.
    pub(crate) loops: u32,
}

/// Budget charge per entry.
pub(crate) const TABLE_ENTRY_BYTES: usize = 40;

impl OrderedTable {
    /// Live entries.
    pub(crate) fn len(&self) -> usize {
        self.live as usize
    }

    /// Positions, holes included: the end for iterators.
    pub(crate) fn extent(&self) -> usize {
        self.entries.len()
    }

    /// Every key and value, holes skipped, for the collector.
    pub(crate) fn values(&self) -> impl Iterator<Item = Value> + '_ {
        self.entries
            .iter()
            .filter(|e| e.key != Value::UNINITIALIZED)
            .flat_map(|e| [e.key, e.value])
    }

    /// The live entry at `position`, if it is not a hole.
    pub(crate) fn at(&self, position: usize) -> Option<(Value, Value)> {
        self.entries
            .get(position)
            .filter(|e| e.key != Value::UNINITIALIZED)
            .map(|e| (e.key, e.value))
    }

    fn find(&self, agent: &Agent, key: Value, hash: u64) -> Option<u32> {
        let mut i = *self.buckets.get(&hash)?;
        while i != NONE {
            let entry = &self.entries[i as usize];
            if agent.same_value_zero(entry.key, key) {
                return Some(i);
            }
            i = entry.next;
        }
        None
    }

    pub(crate) fn get(&self, agent: &Agent, key: Value) -> Option<Value> {
        let key = normalize(key);
        self.find(agent, key, hash(agent, key))
            .map(|i| self.entries[i as usize].value)
    }

    pub(crate) fn has(&self, agent: &Agent, key: Value) -> bool {
        let key = normalize(key);
        self.find(agent, key, hash(agent, key)).is_some()
    }

    /// Sets `key` to `value`; true when the key is new.
    pub(crate) fn set(&mut self, agent: &Agent, key: Value, value: Value) -> bool {
        let key = normalize(key);
        let h = hash(agent, key);
        if let Some(i) = self.find(agent, key, h) {
            self.entries[i as usize].value = value;
            return false;
        }
        self.compact_if_sparse(agent);
        // The heap budget keeps a table far below 2³² entries.
        let index = self.entries.len() as u32;
        let next = self.buckets.insert(h, index).unwrap_or(NONE);
        self.entries.push(Entry { key, value, next });
        self.live += 1;
        true
    }

    /// Removes `key`; true when it was present.
    pub(crate) fn delete(&mut self, agent: &Agent, key: Value) -> bool {
        let key = normalize(key);
        let h = hash(agent, key);
        let Some(i) = self.find(agent, key, h) else {
            return false;
        };
        self.unlink(h, i);
        let entry = &mut self.entries[i as usize];
        entry.key = Value::UNINITIALIZED;
        entry.value = Value::UNDEFINED;
        entry.next = NONE;
        self.live -= 1;
        self.compact_if_sparse(agent);
        true
    }

    fn unlink(&mut self, h: u64, i: u32) {
        let next = self.entries[i as usize].next;
        let head = self.buckets[&h];
        if head == i {
            if next == NONE {
                self.buckets.remove(&h);
            } else {
                self.buckets.insert(h, next);
            }
            return;
        }
        let mut at = head;
        while self.entries[at as usize].next != i {
            at = self.entries[at as usize].next;
        }
        self.entries[at as usize].next = next;
    }

    /// Removes every entry. Open iterators see no more entries.
    pub(crate) fn clear(&mut self) {
        if self.iterators == 0 && self.loops == 0 {
            self.entries = Vec::new();
        } else {
            for e in &mut self.entries {
                e.key = Value::UNINITIALIZED;
                e.value = Value::UNDEFINED;
                e.next = NONE;
            }
        }
        self.buckets = HashMap::new();
        self.live = 0;
    }

    /// Squeezes out holes when they outnumber live entries and nothing
    /// holds a position.
    pub(crate) fn compact_if_sparse(&mut self, agent: &Agent) {
        let holes = self.entries.len() - self.live as usize;
        if self.iterators > 0 || self.loops > 0 || holes <= 8 || holes <= self.live as usize {
            return;
        }
        let old = std::mem::take(&mut self.entries);
        self.buckets.clear();
        self.entries.reserve_exact(self.live as usize);
        for e in old.into_iter().filter(|e| e.key != Value::UNINITIALIZED) {
            let h = hash(agent, e.key);
            let index = self.entries.len() as u32;
            let next = self.buckets.insert(h, index).unwrap_or(NONE);
            self.entries.push(Entry {
                key: e.key,
                value: e.value,
                next,
            });
        }
        self.buckets.shrink_to_fit();
    }

    /// Bytes the collector charges for the table.
    pub(crate) fn footprint(&self) -> usize {
        TABLE_ENTRY_BYTES * self.entries.len()
    }
}

/// -0 is stored as +0 (§24.1.3.9 step 5).
fn normalize(key: Value) -> Value {
    match key.as_number() {
        // A float pattern compares with `==`, so -0 matches too.
        Some(0.0) => Value::number(0.0),
        _ => key,
    }
}

fn hash(agent: &Agent, key: Value) -> u64 {
    match key.kind() {
        Kind::String(s) => agent.heap.content_hash(agent.heap.string(s)),
        Kind::Number(n) if n.is_nan() => 0x7FF8_0000_0000_0000,
        Kind::Number(n) => n.to_bits(),
        // BigInts compare by value; they share one bucket.
        Kind::BigInt(_) => 0xB161_7B16,
        _ => key.bits(),
    }
}
