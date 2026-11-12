//! Environment records (§9.1) and Reference Records (§6.2.5).
//!
//! Environments live in the heap and are named by [`EnvId`] so closures can
//! share them and the collector (JS-03) can trace them. A declarative
//! record keeps names and values in two parallel vectors; an uninitialized
//! binding holds [`Value::UNINITIALIZED`], so the temporal dead zone costs
//! nothing extra (ADR 0015).

use std::rc::Rc;

use super::agent::{Agent, ErrorKind, JsResult};
use super::object::PropertyDescriptor;
use super::shape::PropertyKey;
use super::value::{ObjectId, StringId, Value};

/// An environment in a heap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EnvId(pub(crate) u32);

const MUTABLE: u8 = 1;
const STRICT: u8 = 2;
const DELETABLE: u8 = 4;

/// Names and values of a declarative record.
#[derive(Clone, Debug, Default)]
pub struct Declarative {
    /// Names and flags. Scope environments of compiled code share the
    /// list their function's code owns, so an environment pays only for
    /// its values; the rare binding created or deleted at run time copies
    /// the list first.
    names: Rc<Vec<(StringId, u8)>>,
    values: Vec<Value>,
}

impl Declarative {
    fn find(&self, name: StringId) -> Option<usize> {
        self.names.iter().position(|(n, _)| *n == name)
    }

    fn create(&mut self, name: StringId, flags: u8) {
        Rc::make_mut(&mut self.names).push((name, flags));
        self.values.push(Value::UNINITIALIZED);
    }

    fn remove(&mut self, at: usize) {
        Rc::make_mut(&mut self.names).remove(at);
        self.values.remove(at);
    }
}

/// `[[ThisBindingStatus]]` of a function environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThisStatus {
    /// Arrow functions: `this` comes from the outer environment.
    Lexical,
    /// Not bound yet (derived constructors before `super()`).
    Uninitialized,
    /// Bound to a value.
    Initialized(Value),
}

/// What a function environment holds beyond its bindings.
#[derive(Clone, Debug)]
pub struct FunctionExtra {
    /// `[[ThisBindingStatus]]` and `[[ThisValue]]`.
    pub this: ThisStatus,
    /// `[[FunctionObject]]`.
    pub function: ObjectId,
    /// `[[NewTarget]]`.
    pub new_target: Value,
}

/// An environment record.
#[derive(Clone, Debug)]
pub enum Environment {
    /// Blocks, catch clauses, modules' lexical scope.
    Declarative {
        /// `[[OuterEnv]]`.
        outer: Option<EnvId>,
        /// Bindings.
        record: Declarative,
    },
    /// A function's top-level scope.
    Function {
        /// `[[OuterEnv]]`.
        outer: Option<EnvId>,
        /// Bindings.
        record: Declarative,
        /// The fields only function environments have, boxed so that
        /// block scopes (the common record) stay small.
        extra: Box<FunctionExtra>,
    },
    /// `with` statements and the global object's half of the global
    /// environment.
    Object {
        /// `[[OuterEnv]]`.
        outer: Option<EnvId>,
        /// `[[BindingObject]]`.
        object: ObjectId,
        /// `[[IsWithEnvironment]]`.
        with: bool,
    },
    /// The global environment.
    Global {
        /// `[[ObjectRecord]]`'s binding object.
        object: ObjectId,
        /// `[[DeclarativeRecord]]`.
        record: Declarative,
        /// `[[GlobalThisValue]]`.
        this: ObjectId,
        /// `[[VarNames]]`.
        var_names: Box<Vec<StringId>>,
    },
}

impl Environment {
    /// Report every heap reference this record holds (JS-03).
    pub(crate) fn trace(&self, tracer: &mut impl super::gc::Tracer) {
        let record = match self {
            Environment::Declarative { outer, record } => {
                tracer.env_opt(*outer);
                record
            }
            Environment::Function {
                outer,
                record,
                extra,
            } => {
                tracer.env_opt(*outer);
                tracer.value(Value::object(extra.function));
                tracer.value(extra.new_target);
                if let ThisStatus::Initialized(v) = extra.this {
                    tracer.value(v);
                }
                record
            }
            Environment::Object { outer, object, .. } => {
                tracer.env_opt(*outer);
                tracer.value(Value::object(*object));
                return;
            }
            Environment::Global {
                object,
                record,
                this,
                var_names,
            } => {
                tracer.value(Value::object(*object));
                tracer.value(Value::object(*this));
                for name in var_names.iter() {
                    tracer.string(*name);
                }
                record
            }
        };
        for (name, _) in record.names.iter() {
            tracer.string(*name);
        }
        for value in &record.values {
            tracer.value(*value);
        }
    }

    /// Bytes this record is charged for (see `gc::footprint`).
    pub(crate) fn footprint(&self) -> usize {
        // Eight bytes per value, and eight more per name unless the list
        // belongs to compiled code, which is charged for it there.
        let bindings = match self {
            Environment::Declarative { record, .. }
            | Environment::Function { record, .. }
            | Environment::Global { record, .. } => {
                let per_binding = if Rc::strong_count(&record.names) == 1 {
                    16
                } else {
                    8
                };
                per_binding * record.names.len()
            }
            Environment::Object { .. } => 0,
        };
        let boxed = match self {
            Environment::Function { .. } => std::mem::size_of::<FunctionExtra>(),
            Environment::Global { .. } => std::mem::size_of::<Vec<StringId>>(),
            _ => 0,
        };
        std::mem::size_of::<Environment>() + boxed + bindings
    }

    fn outer(&self) -> Option<EnvId> {
        match self {
            Environment::Declarative { outer, .. }
            | Environment::Function { outer, .. }
            | Environment::Object { outer, .. } => *outer,
            Environment::Global { .. } => None,
        }
    }

    fn declarative(&self) -> Option<&Declarative> {
        match self {
            Environment::Declarative { record, .. }
            | Environment::Function { record, .. }
            | Environment::Global { record, .. } => Some(record),
            Environment::Object { .. } => None,
        }
    }

    fn declarative_mut(&mut self) -> Option<&mut Declarative> {
        match self {
            Environment::Declarative { record, .. }
            | Environment::Function { record, .. }
            | Environment::Global { record, .. } => Some(record),
            Environment::Object { .. } => None,
        }
    }
}

/// The base of a reference.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReferenceBase {
    /// An unresolvable identifier.
    Unresolvable,
    /// A property reference on a value.
    Value(Value),
    /// A binding in an environment.
    Environment(EnvId),
}

/// A Reference Record (§6.2.5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reference {
    /// `[[Base]]`.
    pub base: ReferenceBase,
    /// `[[ReferencedName]]`: a binding name is a string key.
    pub name: PropertyKey,
    /// `[[Strict]]`.
    pub strict: bool,
    /// `[[ThisValue]]` (`super.x` references).
    pub this_value: Option<Value>,
}

impl Agent {
    fn alloc_env(&mut self, env: Environment) -> JsResult<EnvId> {
        Ok(self.heap.alloc_env(env)?)
    }

    fn env(&self, id: EnvId) -> &Environment {
        self.heap.env(id)
    }

    /// `NewDeclarativeEnvironment(outer)`.
    pub fn new_declarative_environment(&mut self, outer: Option<EnvId>) -> JsResult<EnvId> {
        self.alloc_env(Environment::Declarative {
            outer,
            record: Declarative::default(),
        })
    }

    /// `NewObjectEnvironment(O, W, outer)`.
    pub fn new_object_environment(
        &mut self,
        object: ObjectId,
        with: bool,
        outer: Option<EnvId>,
    ) -> JsResult<EnvId> {
        self.alloc_env(Environment::Object {
            outer,
            object,
            with,
        })
    }

    /// `NewFunctionEnvironment(F, newTarget)`, with `this` lexical for
    /// arrow functions.
    pub fn new_function_environment(
        &mut self,
        function: ObjectId,
        outer: Option<EnvId>,
        lexical_this: bool,
        new_target: Value,
    ) -> JsResult<EnvId> {
        let this = if lexical_this {
            ThisStatus::Lexical
        } else {
            ThisStatus::Uninitialized
        };
        self.heap.charge(std::mem::size_of::<FunctionExtra>())?;
        self.alloc_env(Environment::Function {
            outer,
            record: Declarative::default(),
            extra: Box::new(FunctionExtra {
                this,
                function,
                new_target,
            }),
        })
    }

    /// `NewGlobalEnvironment(G, thisValue)`.
    pub fn new_global_environment(&mut self, global: ObjectId, this: ObjectId) -> JsResult<EnvId> {
        self.heap.charge(std::mem::size_of::<Vec<StringId>>())?;
        self.alloc_env(Environment::Global {
            object: global,
            record: Declarative::default(),
            this,
            var_names: Box::default(),
        })
    }

    /// The outer environment.
    pub fn outer_environment(&self, env: EnvId) -> Option<EnvId> {
        self.env(env).outer()
    }

    fn unscopable(&mut self, object: ObjectId, name: StringId) -> JsResult<bool> {
        let unscopables = self.get(object, PropertyKey::Symbol(self.symbols.unscopables))?;
        match unscopables.as_object() {
            Some(u) => {
                let blocked = self.get(u, PropertyKey::String(name))?;
                Ok(self.to_boolean(blocked))
            }
            None => Ok(false),
        }
    }

    /// `HasBinding(N)`.
    pub fn has_binding(&mut self, env: EnvId, name: StringId) -> JsResult<bool> {
        match self.env(env).clone_shallow() {
            Shallow::Declarative => Ok(self
                .env(env)
                .declarative()
                .and_then(|d| d.find(name))
                .is_some()),
            Shallow::Object { object, with } => {
                if !self.has_property(object, PropertyKey::String(name))? {
                    return Ok(false);
                }
                if !with {
                    return Ok(true);
                }
                Ok(!self.unscopable(object, name)?)
            }
            Shallow::Global { object } => {
                if self
                    .env(env)
                    .declarative()
                    .and_then(|d| d.find(name))
                    .is_some()
                {
                    return Ok(true);
                }
                self.has_property(object, PropertyKey::String(name))
            }
        }
    }

    fn declarative_mut(&mut self, env: EnvId) -> &mut Declarative {
        self.heap
            .env_mut(env)
            .declarative_mut()
            .expect("caller checked that the environment is declarative")
    }

    /// `CreateMutableBinding(N, D)`.
    pub fn create_mutable_binding(
        &mut self,
        env: EnvId,
        name: StringId,
        deletable: bool,
    ) -> JsResult<()> {
        match self.env(env).clone_shallow() {
            Shallow::Object { object, .. } => {
                let desc = PropertyDescriptor::data_with(Value::UNDEFINED, true, true, deletable);
                self.define_property_or_throw(object, PropertyKey::String(name), desc)
            }
            _ => {
                self.heap.charge(16)?;
                let flags = MUTABLE | if deletable { DELETABLE } else { 0 };
                self.declarative_mut(env).create(name, flags);
                Ok(())
            }
        }
    }

    /// `CreateImmutableBinding(N, S)`.
    pub fn create_immutable_binding(
        &mut self,
        env: EnvId,
        name: StringId,
        strict: bool,
    ) -> JsResult<()> {
        self.heap.charge(16)?;
        self.declarative_mut(env)
            .create(name, if strict { STRICT } else { 0 });
        Ok(())
    }

    /// `InitializeBinding(N, V)`.
    pub fn initialize_binding(&mut self, env: EnvId, name: StringId, value: Value) -> JsResult<()> {
        if let Shallow::Object { object, .. } = self.env(env).clone_shallow() {
            return self.set(object, PropertyKey::String(name), value, false);
        }
        let in_record = self.env(env).declarative().and_then(|d| d.find(name));
        match (in_record, self.env(env).clone_shallow()) {
            (Some(at), _) => {
                self.declarative_mut(env).values[at] = value;
                Ok(())
            }
            (None, Shallow::Global { object }) => {
                self.set(object, PropertyKey::String(name), value, false)
            }
            (None, _) => Err(self.throw(ErrorKind::ReferenceError, "binding does not exist")),
        }
    }

    /// `SetMutableBinding(N, V, S)`.
    pub fn set_mutable_binding(
        &mut self,
        env: EnvId,
        name: StringId,
        value: Value,
        strict: bool,
    ) -> JsResult<()> {
        let shallow = self.env(env).clone_shallow();
        if let Shallow::Object { object, .. } = shallow {
            let key = PropertyKey::String(name);
            if !self.has_property(object, key)? && strict {
                return Err(self.reference_error_for(name, "is not defined"));
            }
            return self.set(object, key, value, strict);
        }
        let Some(at) = self.env(env).declarative().and_then(|d| d.find(name)) else {
            if let Shallow::Global { object } = shallow {
                let key = PropertyKey::String(name);
                if !self.has_property(object, key)? && strict {
                    return Err(self.reference_error_for(name, "is not defined"));
                }
                return self.set(object, key, value, strict);
            }
            // §9.1.1.1.5 step 1: create the binding in sloppy code.
            if strict {
                return Err(self.reference_error_for(name, "is not defined"));
            }
            self.create_mutable_binding(env, name, true)?;
            return self.initialize_binding(env, name, value);
        };
        let record = self
            .env(env)
            .declarative()
            .expect("found in a declarative record");
        let flags = record.names[at].1;
        let strict = strict || flags & STRICT != 0;
        if record.values[at] == Value::UNINITIALIZED {
            return Err(self.reference_error_for(name, "cannot be accessed before initialization"));
        }
        if flags & MUTABLE != 0 {
            self.declarative_mut(env).values[at] = value;
            Ok(())
        } else if strict {
            self.type_error("assignment to a constant binding")
        } else {
            Ok(())
        }
    }

    /// `GetBindingValue(N, S)`.
    pub fn get_binding_value(
        &mut self,
        env: EnvId,
        name: StringId,
        strict: bool,
    ) -> JsResult<Value> {
        let shallow = self.env(env).clone_shallow();
        if let Shallow::Object { object, .. } = shallow {
            return self.object_binding_value(object, name, strict);
        }
        match self.env(env).declarative().and_then(|d| d.find(name)) {
            Some(at) => {
                let value = self.env(env).declarative().expect("found above").values[at];
                if value == Value::UNINITIALIZED {
                    return Err(
                        self.reference_error_for(name, "cannot be accessed before initialization")
                    );
                }
                Ok(value)
            }
            None => match shallow {
                Shallow::Global { object } => self.object_binding_value(object, name, strict),
                _ => Err(self.reference_error_for(name, "is not defined")),
            },
        }
    }

    fn object_binding_value(
        &mut self,
        object: ObjectId,
        name: StringId,
        strict: bool,
    ) -> JsResult<Value> {
        let key = PropertyKey::String(name);
        if !self.has_property(object, key)? {
            if strict {
                return Err(self.reference_error_for(name, "is not defined"));
            }
            return Ok(Value::UNDEFINED);
        }
        self.get(object, key)
    }

    /// `DeleteBinding(N)`.
    pub fn delete_binding(&mut self, env: EnvId, name: StringId) -> JsResult<bool> {
        let shallow = self.env(env).clone_shallow();
        if let Shallow::Object { object, .. } = shallow {
            return self.delete(object, PropertyKey::String(name));
        }
        match self.env(env).declarative().and_then(|d| d.find(name)) {
            Some(at) => {
                let flags = self.env(env).declarative().expect("found above").names[at].1;
                if flags & DELETABLE == 0 {
                    return Ok(false);
                }
                self.declarative_mut(env).remove(at);
                Ok(true)
            }
            None => match shallow {
                Shallow::Global { object } => {
                    let key = PropertyKey::String(name);
                    if !self.has_own_property(object, key)? {
                        return Ok(true);
                    }
                    let deleted = self.delete(object, key)?;
                    if deleted {
                        if let Environment::Global { var_names, .. } = self.heap.env_mut(env) {
                            var_names.retain(|n| *n != name);
                        }
                    }
                    Ok(deleted)
                }
                _ => Ok(true),
            },
        }
    }

    /// `HasThisBinding()`.
    pub fn has_this_binding(&self, env: EnvId) -> bool {
        match self.env(env) {
            Environment::Function { extra, .. } => extra.this != ThisStatus::Lexical,
            Environment::Global { .. } => true,
            _ => false,
        }
    }

    /// `BindThisValue(V)` of a function environment.
    pub fn bind_this_value(&mut self, env: EnvId, value: Value) -> JsResult<()> {
        let status = match self.env(env) {
            Environment::Function { extra, .. } => extra.this,
            _ => return self.type_error("environment has no this binding"),
        };
        if status != ThisStatus::Uninitialized {
            return Err(self.throw(ErrorKind::ReferenceError, "this is already initialized"));
        }
        if let Environment::Function { extra, .. } = self.heap.env_mut(env) {
            extra.this = ThisStatus::Initialized(value);
        }
        Ok(())
    }

    /// `GetThisBinding()`.
    pub fn get_this_binding(&mut self, env: EnvId) -> JsResult<Value> {
        let status = match self.env(env) {
            Environment::Function { extra, .. } => extra.this,
            Environment::Global { this, .. } => return Ok(Value::object(*this)),
            _ => ThisStatus::Lexical,
        };
        match status {
            ThisStatus::Initialized(v) => Ok(v),
            ThisStatus::Uninitialized => Err(self.throw(
                ErrorKind::ReferenceError,
                "must call super constructor before accessing this",
            )),
            ThisStatus::Lexical => self.type_error("environment has no this binding"),
        }
    }

    /// `GetThisEnvironment()` from `env`.
    pub fn get_this_environment(&self, mut env: EnvId) -> EnvId {
        loop {
            if self.has_this_binding(env) {
                return env;
            }
            match self.outer_environment(env) {
                Some(outer) => env = outer,
                // A global environment always has a this binding.
                None => return env,
            }
        }
    }

    /// `WithBaseObject()`.
    pub fn with_base_object(&self, env: EnvId) -> Value {
        match self.env(env) {
            Environment::Object {
                object, with: true, ..
            } => Value::object(*object),
            _ => Value::UNDEFINED,
        }
    }

    // ---- global environment (§9.1.1.4) --------------------------------

    /// `HasLexicalDeclaration(N)`.
    pub fn has_lexical_declaration(&self, env: EnvId, name: StringId) -> bool {
        matches!(self.env(env), Environment::Global { record, .. } if record.find(name).is_some())
    }

    /// `HasVarDeclaration(N)`.
    pub fn has_var_declaration(&self, env: EnvId, name: StringId) -> bool {
        matches!(self.env(env), Environment::Global { var_names, .. } if var_names.contains(&name))
    }

    /// `HasRestrictedGlobalProperty(N)`.
    pub fn has_restricted_global_property(&mut self, env: EnvId, name: StringId) -> JsResult<bool> {
        let Environment::Global { object, .. } = *self.env(env) else {
            return Ok(false);
        };
        Ok(self
            .get_own_property(object, PropertyKey::String(name))?
            .is_some_and(|d| d.configurable == Some(false)))
    }

    /// `CanDeclareGlobalVar(N)`.
    pub fn can_declare_global_var(&mut self, global: ObjectId, name: StringId) -> JsResult<bool> {
        if self.has_own_property(global, PropertyKey::String(name))? {
            return Ok(true);
        }
        Ok(self.is_extensible(global))
    }

    /// `CreateGlobalVarBinding(N, D)`.
    pub fn create_global_var_binding(
        &mut self,
        env: EnvId,
        name: StringId,
        deletable: bool,
    ) -> JsResult<()> {
        let Environment::Global { object, .. } = *self.env(env) else {
            return self.type_error("not a global environment");
        };
        let key = PropertyKey::String(name);
        if !self.has_own_property(object, key)? && self.is_extensible(object) {
            let desc = PropertyDescriptor::data_with(Value::UNDEFINED, true, true, deletable);
            self.define_property_or_throw(object, key, desc)?;
        }
        if let Environment::Global { var_names, .. } = self.heap.env_mut(env) {
            if !var_names.contains(&name) {
                var_names.push(name);
            }
        }
        Ok(())
    }

    // ---- references (§6.2.5) ------------------------------------------

    /// `GetIdentifierReference(env, name, strict)` over the chain.
    pub fn resolve_binding(
        &mut self,
        env: EnvId,
        name: StringId,
        strict: bool,
    ) -> JsResult<Reference> {
        let mut current = Some(env);
        while let Some(e) = current {
            if self.has_binding(e, name)? {
                return Ok(Reference {
                    base: ReferenceBase::Environment(e),
                    name: PropertyKey::String(name),
                    strict,
                    this_value: None,
                });
            }
            current = self.outer_environment(e);
        }
        Ok(Reference {
            base: ReferenceBase::Unresolvable,
            name: PropertyKey::String(name),
            strict,
            this_value: None,
        })
    }

    fn binding_name(&mut self, reference: &Reference) -> JsResult<StringId> {
        match reference.name {
            PropertyKey::String(s) => Ok(s),
            _ => self.type_error("binding names are strings"),
        }
    }

    /// `GetValue(V)`.
    pub fn get_value(&mut self, reference: &Reference) -> JsResult<Value> {
        match reference.base {
            ReferenceBase::Unresolvable => {
                let name = self.binding_name(reference)?;
                Err(self.reference_error_for(name, "is not defined"))
            }
            ReferenceBase::Value(base) => {
                let this = reference.this_value.unwrap_or(base);
                match base.as_object() {
                    Some(o) => self.get_with_receiver(o, reference.name, this),
                    None => self.get_v(base, reference.name),
                }
            }
            ReferenceBase::Environment(env) => {
                let name = self.binding_name(reference)?;
                self.get_binding_value(env, name, reference.strict)
            }
        }
    }

    /// `PutValue(V, W)`.
    pub fn put_value(&mut self, reference: &Reference, value: Value) -> JsResult<()> {
        match reference.base {
            ReferenceBase::Unresolvable => {
                let name = self.binding_name(reference)?;
                if reference.strict {
                    return Err(self.reference_error_for(name, "is not defined"));
                }
                let global = self.intrinsics.global;
                self.set(global, reference.name, value, false)
            }
            ReferenceBase::Value(base) => {
                let o = self.to_object(base)?;
                let this = reference.this_value.unwrap_or(base);
                let succeeded = self.set_with_receiver(o, reference.name, value, this)?;
                if !succeeded && reference.strict {
                    return self.type_error("cannot assign to read-only property");
                }
                Ok(())
            }
            ReferenceBase::Environment(env) => {
                let name = self.binding_name(reference)?;
                self.set_mutable_binding(env, name, value, reference.strict)
            }
        }
    }

    /// `InitializeReferencedBinding(V, W)`.
    pub fn initialize_referenced_binding(
        &mut self,
        reference: &Reference,
        value: Value,
    ) -> JsResult<()> {
        match reference.base {
            ReferenceBase::Environment(env) => {
                let name = self.binding_name(reference)?;
                self.initialize_binding(env, name, value)
            }
            _ => self.type_error("not a binding reference"),
        }
    }

    pub(crate) fn reference_error_for(
        &mut self,
        name: StringId,
        what: &str,
    ) -> super::agent::Abrupt {
        let text = format!("{} {what}", self.heap.string(name).to_text());
        self.throw(ErrorKind::ReferenceError, &text)
    }
}

/// The kind of an environment, copied out so the heap can be borrowed
/// mutably afterwards.
#[derive(Clone, Copy)]
enum Shallow {
    Declarative,
    Object { object: ObjectId, with: bool },
    Global { object: ObjectId },
}

impl Environment {
    fn clone_shallow(&self) -> Shallow {
        match self {
            Environment::Declarative { .. } | Environment::Function { .. } => Shallow::Declarative,
            Environment::Object { object, with, .. } => Shallow::Object {
                object: *object,
                with: *with,
            },
            Environment::Global { object, .. } => Shallow::Global { object: *object },
        }
    }
}

// ---- slot access for compiled code (JS-04) -----------------------------

/// Binding flags a compiled scope declares.
pub(crate) const BINDING_MUTABLE: u8 = MUTABLE;
/// Assignment to this immutable binding throws even in sloppy code.
pub(crate) const BINDING_STRICT: u8 = STRICT;

impl Agent {
    /// A declarative environment holding `names` in order, every binding
    /// starting as `initial` (`UNINITIALIZED` for the temporal dead zone).
    /// Compiled code then reads and writes them by slot.
    pub(crate) fn new_scope_environment(
        &mut self,
        outer: Option<EnvId>,
        names: &Rc<Vec<(StringId, u8)>>,
        initial: Value,
    ) -> JsResult<EnvId> {
        self.heap.charge(8 * names.len())?;
        let record = Declarative {
            names: Rc::clone(names),
            values: vec![initial; names.len()],
        };
        Ok(self
            .heap
            .alloc_env(Environment::Declarative { outer, record })?)
    }

    /// A copy of a scope environment with the same outer environment and
    /// current values: the per-iteration bindings of a `for (let ...)`
    /// loop (`CreatePerIterationEnvironment`).
    pub(crate) fn copy_scope_environment(&mut self, env: EnvId) -> JsResult<EnvId> {
        let copy = self.heap.env(env).clone();
        if let Some(record) = copy.declarative() {
            self.heap.charge(8 * record.names.len())?;
        }
        Ok(self.heap.alloc_env(copy)?)
    }

    /// The environment `hops` steps out from `env`.
    pub(crate) fn env_outer_n(&self, mut env: EnvId, hops: u16) -> EnvId {
        for _ in 0..hops {
            env = self
                .heap
                .env(env)
                .outer()
                .expect("compiled scope depth matches the environment chain");
        }
        env
    }

    /// The value in a compiled scope's slot (`UNINITIALIZED` in the TDZ).
    pub(crate) fn env_slot(&self, env: EnvId, slot: u16) -> Value {
        self.heap
            .env(env)
            .declarative()
            .expect("compiled scopes are declarative")
            .values[slot as usize]
    }

    /// The name and flags of a compiled scope's slot.
    pub(crate) fn env_slot_binding(&self, env: EnvId, slot: u16) -> (StringId, u8) {
        self.heap
            .env(env)
            .declarative()
            .expect("compiled scopes are declarative")
            .names[slot as usize]
    }

    /// The position of `name` in the global environment's lexical record
    /// (`let`, `const` and `class` at the top of a script). Positions are
    /// stable: global lexical bindings are never removed.
    pub(crate) fn global_lexical_slot(&self, name: StringId) -> Option<u32> {
        match self.heap.env(self.global_env) {
            Environment::Global { record, .. } => record.find(name).map(|i| i as u32),
            _ => None,
        }
    }

    /// The value and flags of a global lexical binding.
    pub(crate) fn global_lexical(&self, slot: u32) -> (Value, u8) {
        match self.heap.env(self.global_env) {
            Environment::Global { record, .. } => {
                (record.values[slot as usize], record.names[slot as usize].1)
            }
            _ => (Value::UNDEFINED, 0),
        }
    }

    /// Store into a global lexical binding without checks.
    pub(crate) fn set_global_lexical(&mut self, slot: u32, value: Value) {
        if let Environment::Global { record, .. } = self.heap.env_mut(self.global_env) {
            record.values[slot as usize] = value;
        }
    }

    /// A ReferenceError for reading `name` in its temporal dead zone.
    pub(crate) fn tdz_error(&mut self, name: StringId) -> super::agent::Abrupt {
        self.reference_error_for(name, "cannot be accessed before initialization")
    }

    /// A ReferenceError for a name nothing declares.
    pub(crate) fn not_defined_error(&mut self, name: StringId) -> super::agent::Abrupt {
        self.reference_error_for(name, "is not defined")
    }

    /// Store into a compiled scope's slot without checks.
    pub(crate) fn set_env_slot(&mut self, env: EnvId, slot: u16, value: Value) {
        self.declarative_mut(env).values[slot as usize] = value;
    }
}
