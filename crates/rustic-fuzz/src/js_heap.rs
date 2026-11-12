//! The JavaScript heap and collector (`rustic-js` JS-03) against a shadow
//! model.
//!
//! Each input byte picks an operation on a small agent: allocate objects
//! and strings, set and delete properties (named, indexed, far-sparse and
//! symbol keys, and enough names to force dictionary mode), link objects
//! into cycles and prototype chains, drop handles, hold values in host
//! roots, and collect. A plain Rust model records what every object should
//! contain. After each collection, every object the model can still reach
//! must read back exactly as modeled, the live object count must equal the
//! model's reachable set, and a final collection with no roots must return
//! the budget to the bare agent's footprint.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::Target;
use rustic_js::runtime::{Agent, HostRoot, ObjectId, PropertyKey, Roots, SymbolId, Value};

pub(crate) struct JsHeapTarget;

/// Operation streams: a few hand-written ones that reach every opcode,
/// then fixed pseudo-random ones of growing length.
pub(crate) fn seeds() -> Vec<Vec<u8>> {
    let mut seeds = vec![
        (0u8..=255).collect(),
        [0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14].repeat(8),
        b"\x00\x00\x00\x03\x01\x07\x05\x09\x02\x0b\x0c\x0d\x0e\x0f".repeat(16),
        [0, 3, 10, 9, 0, 6, 9, 12, 13, 9].repeat(20),
    ];
    let mut rng = crate::Rng::new(0x006a_732d_6865_6170);
    for n in 0..124usize {
        let len = 16 + n * 4;
        seeds.push(
            (0..len)
                .map(|_| u8::try_from(rng.below(256)).unwrap_or(0))
                .collect(),
        );
    }
    seeds
}

#[derive(Clone, Debug, PartialEq)]
enum Model {
    Number(f64),
    Text(String),
    Object(ObjectId),
}

/// Keys the operations draw from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum ModelKey {
    Name(u8),
    Index(u32),
    Symbol(u8),
}

struct Run {
    agent: Agent,
    symbols: Vec<SymbolId>,
    held: Vec<ObjectId>,
    host: Vec<(HostRoot, ObjectId)>,
    /// Every object the model knows: its properties and prototype.
    objects: HashMap<ObjectId, (BTreeMap<ModelKey, Model>, Option<ObjectId>)>,
    intrinsic_objects: usize,
    baseline: usize,
    counter: u32,
}

impl Run {
    fn key(&mut self, key: ModelKey) -> Result<PropertyKey, String> {
        Ok(match key {
            ModelKey::Name(n) => self
                .agent
                .heap
                .key(&format!("name{n}"))
                .map_err(|_| "out of memory")?,
            ModelKey::Index(i) => PropertyKey::Index(i),
            ModelKey::Symbol(s) => PropertyKey::Symbol(self.symbols[s as usize]),
        })
    }

    fn pick(&self, byte: u8) -> Option<ObjectId> {
        (!self.held.is_empty()).then(|| self.held[byte as usize % self.held.len()])
    }

    fn value_of(&mut self, model: &Model) -> Result<Value, String> {
        Ok(match model {
            Model::Number(n) => Value::number(*n),
            Model::Text(t) => self.agent.string(t).map_err(|e| format!("{e:?}"))?,
            Model::Object(o) => Value::object(*o),
        })
    }

    fn set(&mut self, o: ObjectId, key: ModelKey, model: Model) -> Result<(), String> {
        let k = self.key(key)?;
        let v = self.value_of(&model)?;
        self.agent
            .create_data_property_or_throw(o, k, v)
            .map_err(|e| format!("set failed: {e:?}"))?;
        self.objects
            .get_mut(&o)
            .ok_or("unknown object")?
            .0
            .insert(key, model);
        Ok(())
    }

    fn step(&mut self, op: u8, arg: u8) -> Result<(), String> {
        self.counter += 1;
        match op % 15 {
            0 => {
                let o = self.agent.new_object().map_err(|e| format!("{e:?}"))?;
                let proto = self.agent.intrinsics.object_prototype;
                self.objects.insert(o, (BTreeMap::new(), Some(proto)));
                self.held.push(o);
            }
            1 | 2 => {
                if let Some(o) = self.pick(arg) {
                    let key = ModelKey::Name(arg % 8);
                    self.set(o, key, Model::Number(f64::from(self.counter)))?;
                }
            }
            3 => {
                if let (Some(o), Some(target)) = (self.pick(arg), self.pick(arg / 3)) {
                    self.set(o, ModelKey::Name(arg % 5), Model::Object(target))?;
                }
            }
            4 => {
                if let Some(o) = self.pick(arg) {
                    let text = format!("s{}-{arg}", self.counter);
                    self.set(o, ModelKey::Index(u32::from(arg % 4)), Model::Text(text))?;
                }
            }
            5 => {
                if let (Some(o), Some(target)) = (self.pick(arg), self.pick(arg / 5)) {
                    self.set(
                        o,
                        ModelKey::Index(1_000_000 + u32::from(arg)),
                        Model::Object(target),
                    )?;
                }
            }
            6 => {
                if let Some(o) = self.pick(arg) {
                    let key = ModelKey::Symbol(arg % 2);
                    self.set(o, key, Model::Number(-f64::from(arg)))?;
                }
            }
            7 => {
                // Enough names to leave the shape tree.
                if let Some(o) = self.pick(arg) {
                    for n in 0..40u8 {
                        self.set(o, ModelKey::Name(8 + n), Model::Number(f64::from(n)))?;
                    }
                }
            }
            8 => {
                if let Some(o) = self.pick(arg) {
                    let key = ModelKey::Name(arg % 8);
                    let k = self.key(key)?;
                    let deleted = self.agent.delete(o, k).map_err(|e| format!("{e:?}"))?;
                    if !deleted {
                        return Err("configurable property refused deletion".into());
                    }
                    self.objects
                        .get_mut(&o)
                        .ok_or("unknown object")?
                        .0
                        .remove(&key);
                }
            }
            9 => {
                if !self.held.is_empty() {
                    let at = arg as usize % self.held.len();
                    self.held.swap_remove(at);
                }
            }
            10 => {
                if let (Some(o), Some(proto)) = (self.pick(arg), self.pick(arg / 7)) {
                    if self.agent.set_prototype_of(o, Some(proto)).unwrap() {
                        self.objects.get_mut(&o).ok_or("unknown object")?.1 = Some(proto);
                    }
                }
            }
            11 => {
                if let Some(o) = self.pick(arg) {
                    let root = self
                        .agent
                        .root(Value::object(o))
                        .map_err(|e| format!("{e:?}"))?;
                    self.host.push((root, o));
                }
            }
            12 => {
                if !self.host.is_empty() {
                    let (root, _) = self.host.swap_remove(arg as usize % self.host.len());
                    if !self.agent.unroot(root) || self.agent.rooted(root).is_some() {
                        return Err("unroot did not release the handle".into());
                    }
                }
            }
            13 => self.collect_and_check()?,
            _ => {
                // Garbage: strings and objects nobody holds.
                for i in 0..u32::from(arg % 16) {
                    self.agent
                        .string(&format!("garbage {i}"))
                        .map_err(|e| format!("{e:?}"))?;
                    let o = self.agent.new_object().map_err(|e| format!("{e:?}"))?;
                    self.objects.insert(o, (BTreeMap::new(), None));
                }
            }
        }
        Ok(())
    }

    /// Objects the model can reach from held handles and host roots.
    fn reachable(&self) -> HashSet<ObjectId> {
        let mut seen = HashSet::new();
        let mut work: Vec<ObjectId> = self.held.clone();
        work.extend(self.host.iter().map(|(_, o)| *o));
        while let Some(o) = work.pop() {
            if !seen.insert(o) {
                continue;
            }
            let (properties, proto) = &self.objects[&o];
            for value in properties.values() {
                if let Model::Object(target) = value {
                    work.push(*target);
                }
            }
            if let Some(proto) = proto {
                if self.objects.contains_key(proto) {
                    work.push(*proto);
                }
            }
        }
        seen
    }

    fn collect_and_check(&mut self) -> Result<(), String> {
        let held: Vec<Value> = self.held.iter().map(|&o| Value::object(o)).collect();
        self.agent.collect_garbage(Roots {
            values: &held,
            envs: &[],
            codes: &[],
        });
        let live = self.reachable();
        self.objects.retain(|o, _| live.contains(o));
        if self.agent.heap.object_count() != self.intrinsic_objects + live.len() {
            return Err(format!(
                "{} live objects after collection, model expects {}",
                self.agent.heap.object_count(),
                self.intrinsic_objects + live.len()
            ));
        }
        let snapshot: Vec<(ObjectId, BTreeMap<ModelKey, Model>, Option<ObjectId>)> = self
            .objects
            .iter()
            .map(|(o, (p, proto))| (*o, p.clone(), *proto))
            .collect();
        for (o, properties, proto) in snapshot {
            if self.agent.get_prototype_of(o).unwrap() != proto {
                return Err(format!("prototype of {o:?} changed"));
            }
            for (key, expected) in properties {
                let k = self.key(key)?;
                let got = self.agent.get(o, k).map_err(|e| format!("{e:?}"))?;
                let matches = match &expected {
                    Model::Number(n) => got == Value::number(*n),
                    Model::Object(target) => got == Value::object(*target),
                    Model::Text(t) => {
                        got.as_string().map(|s| self.agent.heap.string(s).to_text())
                            == Some(t.clone())
                    }
                };
                if !matches {
                    return Err(format!("{o:?}.{key:?} read {got:?}, expected {expected:?}"));
                }
            }
        }
        Ok(())
    }
}

impl Target for JsHeapTarget {
    fn name(&self) -> &'static str {
        "js-heap"
    }

    fn max_len(&self) -> usize {
        1024
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"\x00\x00\x00",
            b"\x0d",
            b"\x09\x09",
            b"\x07",
            b"\x0b\x0d\x0c\x0d",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let mut agent = Agent::with_heap_limit(64 << 20).map_err(|e| format!("{e:?}"))?;
        let baseline = agent.collect_garbage(Roots::default()).bytes_after;
        let mut symbols = Vec::new();
        let mut symbol_roots = Vec::new();
        for _ in 0..2 {
            let symbol = agent
                .heap
                .alloc_symbol(None)
                .map_err(|e| format!("{e:?}"))?;
            // Handles held in Rust must be rooted to survive a collection.
            symbol_roots.push(
                agent
                    .root(Value::symbol(symbol))
                    .map_err(|e| format!("{e:?}"))?,
            );
            symbols.push(symbol);
        }
        let intrinsic_objects = agent.heap.object_count();
        let mut run = Run {
            agent,
            symbols,
            held: Vec::new(),
            host: Vec::new(),
            objects: HashMap::new(),
            intrinsic_objects,
            baseline,
            counter: 0,
        };
        for pair in input.chunks(2) {
            run.step(pair[0], pair.get(1).copied().unwrap_or(0))?;
        }
        run.collect_and_check()?;
        // With nothing held, only the intrinsics remain.
        run.held.clear();
        for root in symbol_roots {
            run.agent.unroot(root);
        }
        for (root, _) in std::mem::take(&mut run.host) {
            run.agent.unroot(root);
        }
        run.agent.collect_garbage(Roots::default());
        if run.agent.heap.bytes_used() != run.baseline {
            return Err(format!(
                "{} bytes charged after collecting everything, baseline {}",
                run.agent.heap.bytes_used(),
                run.baseline
            ));
        }
        Ok(())
    }
}
