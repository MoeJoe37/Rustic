//! The agent: one heap, its well-known names and symbols, and the
//! intrinsic objects of its realm (§9.3, §9.7). Built-in library objects
//! beyond what JS-02's conversions need arrive with JS-05.

use std::num::NonZeroU64;

use rustic_base::IdSequence;

use super::env::EnvId;
use super::heap::{DEFAULT_HEAP_BYTES, Heap, OutOfMemory};
use super::number::number_to_string;
use super::object::{Class, Object, PropertyDescriptor};
use super::shape::PropertyKey;
use super::string::JsStr;
use super::value::{Kind, ObjectId, StringId, SymbolId, Value};
use crate::vm::interp::VmState;

/// Why an operation did not complete normally.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Abrupt {
    /// A `throw` completion with the thrown value.
    Throw(Value),
    /// The heap budget ran out. Not catchable by script.
    OutOfMemory,
    /// The host cancelled execution. Not catchable by script.
    Cancelled,
}

impl From<OutOfMemory> for Abrupt {
    fn from(_: OutOfMemory) -> Self {
        Abrupt::OutOfMemory
    }
}

/// The result of an operation that may throw.
pub type JsResult<T> = Result<T, Abrupt>;

/// A built-in function: `(agent, this, arguments) -> completion`.
pub type NativeFn = fn(&mut Agent, Value, &[Value]) -> JsResult<Value>;

/// The native error constructors' kinds (§20.5.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// `Error`.
    Error,
    /// `TypeError`.
    TypeError,
    /// `RangeError`.
    RangeError,
    /// `ReferenceError`.
    ReferenceError,
    /// `SyntaxError`.
    SyntaxError,
}

macro_rules! names {
    ($($field:ident = $text:literal,)*) => {
        /// Interned property names the runtime uses.
        #[derive(Clone, Copy, Debug)]
        pub struct Names {
            $(
                #[doc = concat!("`", $text, "`.")]
                pub $field: StringId,
            )*
        }

        impl Names {
            fn new(heap: &mut Heap) -> Result<Self, OutOfMemory> {
                Ok(Self { $($field: heap.intern_str($text)?,)* })
            }

            /// Every name, for the collector's roots.
            pub(crate) fn all(&self) -> impl Iterator<Item = StringId> {
                [$(self.$field,)*].into_iter()
            }
        }
    };
}

names! {
    length = "length",
    prototype = "prototype",
    constructor = "constructor",
    value_of = "valueOf",
    to_string = "toString",
    message = "message",
    name = "name",
    global_this = "globalThis",
    nan = "NaN",
    infinity = "Infinity",
    undefined = "undefined",
    empty = "",
    next = "next",
    done = "done",
    value = "value",
    return_ = "return",
}

macro_rules! symbols {
    ($($field:ident = $text:literal,)*) => {
        /// The well-known symbols (§6.1.5.1).
        #[derive(Clone, Copy, Debug)]
        pub struct WellKnownSymbols {
            $(
                #[doc = concat!("`", $text, "`.")]
                pub $field: SymbolId,
            )*
        }

        impl WellKnownSymbols {
            fn new(heap: &mut Heap) -> Result<Self, OutOfMemory> {
                Ok(Self { $($field: {
                    let description = heap.alloc_str($text)?;
                    heap.alloc_symbol(Some(description))?
                },)* })
            }

            /// Every symbol, for the collector's roots.
            pub(crate) fn all(&self) -> impl Iterator<Item = SymbolId> {
                [$(self.$field,)*].into_iter()
            }
        }
    };
}

symbols! {
    async_iterator = "Symbol.asyncIterator",
    has_instance = "Symbol.hasInstance",
    is_concat_spreadable = "Symbol.isConcatSpreadable",
    iterator = "Symbol.iterator",
    match_ = "Symbol.match",
    match_all = "Symbol.matchAll",
    replace = "Symbol.replace",
    search = "Symbol.search",
    species = "Symbol.species",
    split = "Symbol.split",
    to_primitive = "Symbol.toPrimitive",
    to_string_tag = "Symbol.toStringTag",
    unscopables = "Symbol.unscopables",
    intl_fallback = "IntlLegacyConstructedSymbol",
}

/// The intrinsic objects JS-02 needs.
#[derive(Clone, Copy, Debug)]
pub struct Intrinsics {
    /// `%Object.prototype%`.
    pub object_prototype: ObjectId,
    /// `%Function.prototype%`.
    pub function_prototype: ObjectId,
    /// `%Array.prototype%`.
    pub array_prototype: ObjectId,
    /// `%Array%`.
    pub array: ObjectId,
    /// `%Error.prototype%`.
    pub error_prototype: ObjectId,
    /// `%TypeError.prototype%`.
    pub type_error_prototype: ObjectId,
    /// `%RangeError.prototype%`.
    pub range_error_prototype: ObjectId,
    /// `%ReferenceError.prototype%`.
    pub reference_error_prototype: ObjectId,
    /// `%SyntaxError.prototype%`.
    pub syntax_error_prototype: ObjectId,
    /// `%Boolean.prototype%`.
    pub boolean_prototype: ObjectId,
    /// `%Number.prototype%`.
    pub number_prototype: ObjectId,
    /// `%String.prototype%`.
    pub string_prototype: ObjectId,
    /// `%Symbol.prototype%`.
    pub symbol_prototype: ObjectId,
    /// `%BigInt.prototype%`.
    pub bigint_prototype: ObjectId,
    /// The global object.
    pub global: ObjectId,
    /// `%ArrayIteratorPrototype%`.
    pub array_iterator_prototype: ObjectId,
    /// `%Array.prototype.values%`, also `Array.prototype[Symbol.iterator]`.
    pub array_values: ObjectId,
    /// `%ArrayIteratorPrototype%.next`.
    pub array_iterator_next: ObjectId,
    /// The global symbol registry of `Symbol.for`: a null-prototype
    /// object from keys to symbols, never reachable from script.
    pub symbol_registry: ObjectId,
    /// `%MapIteratorPrototype%` and `%SetIteratorPrototype%`.
    pub table_iterator_prototypes: [ObjectId; 2],
    /// `%IteratorHelperPrototype%` and `%WrapForValidIteratorPrototype%`.
    pub iterator_helper_prototypes: [ObjectId; 2],
    /// `%Iterator%`.
    pub iterator: ObjectId,
    /// `%eval%`: a call of the bare name `eval` that reaches this function
    /// is a direct eval.
    pub eval: ObjectId,
    /// `%GeneratorFunction.prototype%`, the prototype of generator
    /// functions.
    pub generator_function_prototype: ObjectId,
    /// `%GeneratorPrototype%` (`%GeneratorFunction.prototype.prototype%`).
    pub generator_prototype: ObjectId,
    /// `%Promise%`.
    pub promise: ObjectId,
    /// `%Promise.prototype%`.
    pub promise_prototype: ObjectId,
    /// `%AggregateError.prototype%`.
    pub aggregate_error_prototype: ObjectId,
    /// `%AsyncFunction.prototype%`, the prototype of async functions.
    pub async_function_prototype: ObjectId,
    /// `%AsyncGeneratorFunction.prototype%`, the prototype of async
    /// generator functions.
    pub async_generator_function_prototype: ObjectId,
    /// `%AsyncGeneratorPrototype%`.
    pub async_generator_prototype: ObjectId,
    /// `%AsyncFromSyncIteratorPrototype%`.
    pub async_from_sync_iterator_prototype: ObjectId,
    /// `%RegExp%`.
    pub regexp: ObjectId,
    /// `%RegExp.prototype%`.
    pub regexp_prototype: ObjectId,
    /// `%RegExpStringIteratorPrototype%`.
    pub regexp_string_iterator_prototype: ObjectId,
    /// `%ThrowTypeError%` (§10.2.4.1).
    pub throw_type_error: ObjectId,
    /// `%ArrayBuffer%`.
    pub array_buffer: ObjectId,
    /// `%ArrayBuffer.prototype%`.
    pub array_buffer_prototype: ObjectId,
    /// `%DataView.prototype%`.
    pub data_view_prototype: ObjectId,
    /// `%SharedArrayBuffer%`.
    pub shared_array_buffer: ObjectId,
    /// `%SharedArrayBuffer.prototype%`.
    pub shared_array_buffer_prototype: ObjectId,
    /// `%TypedArray%`.
    pub typed_array: ObjectId,
    /// `%TypedArray.prototype%`.
    pub typed_array_prototype: ObjectId,
    /// The typed array constructors, in element kind order.
    pub typed_array_constructors: [ObjectId; 12],
    /// Their prototypes, in the same order.
    pub typed_array_prototypes: [ObjectId; 12],
    /// `%Intl.Locale.prototype%`.
    pub locale_prototype: ObjectId,
    /// `%Intl.ListFormat.prototype%`.
    pub list_format_prototype: ObjectId,
    /// `%Intl.Collator.prototype%`.
    pub collator_prototype: ObjectId,
    /// `%Intl.PluralRules.prototype%`.
    pub plural_rules_prototype: ObjectId,
    /// The prototypes and constructors of the Intl services built on
    /// `builtins::intl::service`, by the indices in `builtins::intl::slot`.
    pub intl: [ObjectId; 16],
}

impl Intrinsics {
    /// Every intrinsic, for the collector's roots.
    pub(crate) fn all(&self) -> Vec<ObjectId> {
        let mut all = vec![
            self.object_prototype,
            self.function_prototype,
            self.array_prototype,
            self.array,
            self.error_prototype,
            self.type_error_prototype,
            self.range_error_prototype,
            self.reference_error_prototype,
            self.syntax_error_prototype,
            self.boolean_prototype,
            self.number_prototype,
            self.string_prototype,
            self.symbol_prototype,
            self.bigint_prototype,
            self.global,
            self.array_iterator_prototype,
            self.array_values,
            self.array_iterator_next,
            self.symbol_registry,
            self.table_iterator_prototypes[0],
            self.table_iterator_prototypes[1],
            self.iterator_helper_prototypes[0],
            self.iterator_helper_prototypes[1],
            self.iterator,
            self.eval,
            self.generator_function_prototype,
            self.generator_prototype,
            self.promise,
            self.promise_prototype,
            self.aggregate_error_prototype,
            self.async_function_prototype,
            self.async_generator_function_prototype,
            self.async_generator_prototype,
            self.async_from_sync_iterator_prototype,
            self.regexp,
            self.regexp_prototype,
            self.regexp_string_iterator_prototype,
            self.throw_type_error,
            self.array_buffer,
            self.array_buffer_prototype,
            self.data_view_prototype,
            self.shared_array_buffer,
            self.shared_array_buffer_prototype,
            self.typed_array,
            self.typed_array_prototype,
        ];
        all.extend(self.typed_array_constructors);
        all.extend(self.typed_array_prototypes);
        all.extend([
            self.locale_prototype,
            self.list_format_prototype,
            self.collator_prototype,
            self.plural_rules_prototype,
        ]);
        all.extend(self.intl);
        all
    }
}

/// A compiled pattern kept for reuse: (source, flags, program).
pub(crate) type RegExpCacheEntry = (
    Vec<u16>,
    crate::regexp::Flags,
    std::rc::Rc<crate::regexp::Regex>,
);

/// Identity of an agent, used to scope host handles to the owning heap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AgentId(NonZeroU64);

static NEXT_AGENT_ID: IdSequence = IdSequence::new();

/// A realm of an agent (§9.3): its own global object and intrinsics.
/// Realm 0 is the one the agent starts in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RealmId(pub(crate) u16);

impl RealmId {
    /// The realm an agent starts in.
    pub const MAIN: RealmId = RealmId(0);
}

/// What a realm that is not running keeps: the running realm's record
/// is in [`Agent::intrinsics`] and the agent's global environment.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RealmRecord {
    pub(crate) intrinsics: Intrinsics,
    pub(crate) global_env: EnvId,
    /// The objects created with the realm are `first_object..` for
    /// `object_count` indices, in the same order in every realm.
    first_object: u32,
    object_count: u32,
}

/// One agent and its realms.
#[derive(Debug)]
pub struct Agent {
    id: AgentId,
    /// All values of the agent.
    pub heap: Heap,
    /// Interned names.
    pub names: Names,
    /// Well-known symbols.
    pub symbols: WellKnownSymbols,
    /// Intrinsic objects of the running realm.
    pub intrinsics: Intrinsics,
    /// The running realm's global environment.
    pub(crate) global_env: EnvId,
    /// Every realm's record by id. The running realm's entry is stale
    /// while it runs: `intrinsics` and `global_env` are written back
    /// when another realm is entered.
    pub(crate) realms: Vec<RealmRecord>,
    /// The interpreter's stack and frames.
    pub(crate) vm: VmState,
    /// Recently compiled patterns, most recent last, so a pattern built
    /// in a loop is parsed and compiled once.
    pub(crate) regexp_cache: Vec<RegExpCacheEntry>,
}

impl Agent {
    /// An agent whose heap may hold [`DEFAULT_HEAP_BYTES`].
    pub fn new() -> Result<Self, OutOfMemory> {
        Self::with_heap_limit(DEFAULT_HEAP_BYTES)
    }

    /// An agent whose heap may hold `limit` bytes.
    pub fn with_heap_limit(limit: usize) -> Result<Self, OutOfMemory> {
        let mut heap = Heap::new(limit);
        let names = Names::new(&mut heap)?;
        let symbols = WellKnownSymbols::new(&mut heap)?;
        let placeholder = ObjectId(0);
        let id = NEXT_AGENT_ID.allocate().map_err(|_| OutOfMemory)?;
        let mut agent = Agent {
            id: AgentId(id),
            heap,
            names,
            symbols,
            intrinsics: Intrinsics {
                object_prototype: placeholder,
                function_prototype: placeholder,
                array_prototype: placeholder,
                array: placeholder,
                error_prototype: placeholder,
                type_error_prototype: placeholder,
                range_error_prototype: placeholder,
                reference_error_prototype: placeholder,
                syntax_error_prototype: placeholder,
                boolean_prototype: placeholder,
                number_prototype: placeholder,
                string_prototype: placeholder,
                symbol_prototype: placeholder,
                bigint_prototype: placeholder,
                global: placeholder,
                array_iterator_prototype: placeholder,
                array_values: placeholder,
                array_iterator_next: placeholder,
                symbol_registry: placeholder,
                table_iterator_prototypes: [placeholder; 2],
                iterator_helper_prototypes: [placeholder; 2],
                iterator: placeholder,
                eval: placeholder,
                generator_function_prototype: placeholder,
                generator_prototype: placeholder,
                promise: placeholder,
                promise_prototype: placeholder,
                aggregate_error_prototype: placeholder,
                async_function_prototype: placeholder,
                async_generator_function_prototype: placeholder,
                async_generator_prototype: placeholder,
                async_from_sync_iterator_prototype: placeholder,
                regexp: placeholder,
                regexp_prototype: placeholder,
                regexp_string_iterator_prototype: placeholder,
                throw_type_error: placeholder,
                array_buffer: placeholder,
                array_buffer_prototype: placeholder,
                data_view_prototype: placeholder,
                shared_array_buffer: placeholder,
                shared_array_buffer_prototype: placeholder,
                typed_array: placeholder,
                typed_array_prototype: placeholder,
                typed_array_constructors: [placeholder; 12],
                typed_array_prototypes: [placeholder; 12],
                locale_prototype: placeholder,
                list_format_prototype: placeholder,
                collator_prototype: placeholder,
                plural_rules_prototype: placeholder,
                intl: [placeholder; 16],
            },
            global_env: EnvId(0),
            realms: Vec::new(),
            vm: VmState::default(),
            regexp_cache: Vec::new(),
        };
        agent.realms.push(RealmRecord {
            intrinsics: agent.intrinsics,
            global_env: agent.global_env,
            first_object: 0,
            object_count: 0,
        });
        agent.initialize_realm(None).map_err(|_| OutOfMemory)?;
        // Prototypes with more methods than a shape holds became
        // dictionaries while they were built; free the shapes they left.
        agent.collect_garbage(super::Roots::default());
        Ok(agent)
    }

    /// The identity of this agent's heap and realms.
    pub fn id(&self) -> AgentId {
        self.id
    }

    // ---- realms ------------------------------------------------------

    /// The running realm.
    pub fn current_realm(&self) -> RealmId {
        RealmId(self.heap.realm)
    }

    /// How many realms the agent has.
    pub fn realm_count(&self) -> usize {
        self.realms.len()
    }

    /// The global object of `realm`.
    pub fn realm_global(&self, realm: RealmId) -> ObjectId {
        self.realm_intrinsics(realm.0).global
    }

    /// The intrinsics of a realm, running or not.
    pub(crate) fn realm_intrinsics(&self, realm: u16) -> &Intrinsics {
        if realm == self.heap.realm {
            &self.intrinsics
        } else {
            &self.realms[usize::from(realm)].intrinsics
        }
    }

    /// Make `realm` the running realm and return the one that was. The
    /// caller enters the returned realm again when its work is done.
    pub fn enter_realm(&mut self, realm: RealmId) -> RealmId {
        let previous = self.heap.realm;
        self.switch_realm(realm.0);
        RealmId(previous)
    }

    #[inline]
    pub(crate) fn switch_realm(&mut self, realm: u16) {
        if realm == self.heap.realm {
            return;
        }
        self.switch_realm_slow(realm);
    }

    #[cold]
    fn switch_realm_slow(&mut self, realm: u16) {
        let running = &mut self.realms[usize::from(self.heap.realm)];
        running.intrinsics = self.intrinsics;
        running.global_env = self.global_env;
        let record = self.realms[usize::from(realm)];
        self.intrinsics = record.intrinsics;
        self.global_env = record.global_env;
        self.heap.realm = realm;
    }

    /// Create the running realm's intrinsics and global environment, and
    /// note which objects they are.
    fn initialize_realm(&mut self, registry: Option<ObjectId>) -> JsResult<()> {
        // The extent is a `u32` index or allocation would have failed.
        let first = self.heap.objects.extent() as u32;
        self.heap.creating_intrinsics = true;
        let created = self.create_intrinsics(registry).and_then(|()| {
            let global = self.intrinsics.global;
            self.global_env = self.new_global_environment(global, global)?;
            Ok(())
        });
        self.heap.creating_intrinsics = false;
        created?;
        let count = self.heap.objects.extent() as u32 - first;
        let record = &mut self.realms[usize::from(self.heap.realm)];
        record.first_object = first;
        record.object_count = count;
        record.intrinsics = self.intrinsics;
        record.global_env = self.global_env;
        debug_assert_eq!(
            count, self.realms[0].object_count,
            "every realm creates the same intrinsics in the same order"
        );
        Ok(())
    }

    /// `InitializeHostDefinedRealm` (§9.6) for another realm of this
    /// agent: a new global object with its own set of intrinsics. The
    /// well-known symbols and the `Symbol.for` registry are the agent's
    /// and shared. The running realm does not change. A realm lives as
    /// long as its agent (TD-118).
    pub fn create_realm(&mut self) -> JsResult<RealmId> {
        let Ok(id) = u16::try_from(self.realms.len()) else {
            return Err(self.throw(ErrorKind::RangeError, "too many realms"));
        };
        let previous = self.heap.realm;
        let registry = self.intrinsics.symbol_registry;
        // Until its own intrinsics exist the record repeats the creating
        // realm's, so a collection in between traces only live objects.
        self.realms.push(RealmRecord {
            intrinsics: self.intrinsics,
            global_env: self.global_env,
            first_object: 0,
            object_count: 0,
        });
        self.switch_realm(id);
        let created = self.initialize_realm(Some(registry));
        self.switch_realm(previous);
        if created.is_err() {
            // Nothing can refer to the half-built realm yet.
            self.realms.pop();
        }
        created.map(|()| RealmId(id))
    }

    /// The object of `realm` that corresponds to `intrinsic` of the
    /// running realm (`%Map.prototype%` for `%Map.prototype%`), or
    /// `intrinsic` itself when it is not one of the running realm's
    /// intrinsics or its counterpart has been collected.
    pub(crate) fn intrinsic_in_realm(&self, intrinsic: ObjectId, realm: u16) -> ObjectId {
        use super::object::FLAG_INTRINSIC;
        if realm == self.heap.realm {
            return intrinsic;
        }
        let from = &self.realms[usize::from(self.heap.realm)];
        let to = &self.realms[usize::from(realm)];
        let Some(offset) = intrinsic.0.checked_sub(from.first_object) else {
            return intrinsic;
        };
        if offset >= from.object_count.min(to.object_count)
            || !self.heap.object(intrinsic).has_flag(FLAG_INTRINSIC)
        {
            return intrinsic;
        }
        let other = to.first_object + offset;
        match self.heap.objects.try_get(other) {
            Some(object) if object.has_flag(FLAG_INTRINSIC) && object.realm == realm => {
                ObjectId(other)
            }
            _ => intrinsic,
        }
    }

    /// `GetFunctionRealm(object)` (§7.3.24): the realm a function was
    /// created in, looking through bound functions and proxies. A
    /// revoked proxy throws a TypeError.
    pub(crate) fn function_realm(&mut self, mut object: ObjectId) -> JsResult<u16> {
        loop {
            let o = self.heap.object(object);
            match o.internal() {
                Some(super::object::Internal::Bound { target, .. }) => object = *target,
                Some(super::object::Internal::Proxy { target, .. }) => match *target {
                    Some(target) => object = target,
                    None => return self.type_error("proxy has been revoked"),
                },
                _ => return Ok(o.realm),
            }
        }
    }

    fn create_intrinsics(&mut self, registry: Option<ObjectId>) -> JsResult<()> {
        let h = &mut self.heap;
        let object_prototype = h.alloc_object(Object::new(Class::Ordinary, None))?;
        let mut function = Object::new(Class::Function, Some(object_prototype));
        function.extra_mut().native = Some(|_, _, _| Ok(Value::UNDEFINED));
        let function_prototype = h.alloc_object(function)?;
        let array_prototype = h.alloc_object(Object::new(Class::Array, Some(object_prototype)))?;
        let error_prototype =
            h.alloc_object(Object::new(Class::Ordinary, Some(object_prototype)))?;
        let mut sub = || h.alloc_object(Object::new(Class::Ordinary, Some(error_prototype)));
        let (
            type_error_prototype,
            range_error_prototype,
            reference_error_prototype,
            syntax_error_prototype,
        ) = (sub()?, sub()?, sub()?, sub()?);
        let empty = self.names.empty;
        let mut wrapper = |class, primitive| {
            let mut object = Object::new(class, Some(object_prototype));
            object.extra_mut().primitive = primitive;
            h.alloc_object(object)
        };
        let boolean_prototype = wrapper(Class::Boolean, Some(Value::FALSE))?;
        let number_prototype = wrapper(Class::Number, Some(Value::number(0.0)))?;
        let string_prototype = wrapper(Class::String, Some(Value::string(empty)))?;
        let symbol_prototype = wrapper(Class::Ordinary, None)?;
        let bigint_prototype = wrapper(Class::Ordinary, None)?;
        let global = h.alloc_object(Object::new(Class::Ordinary, Some(object_prototype)))?;
        self.intrinsics = Intrinsics {
            object_prototype,
            function_prototype,
            array_prototype,
            array: object_prototype,
            error_prototype,
            type_error_prototype,
            range_error_prototype,
            reference_error_prototype,
            syntax_error_prototype,
            boolean_prototype,
            number_prototype,
            string_prototype,
            symbol_prototype,
            bigint_prototype,
            global,
            array_iterator_prototype: object_prototype,
            array_values: object_prototype,
            array_iterator_next: object_prototype,
            symbol_registry: {
                // Allocated in every realm, so that intrinsics keep the
                // same positions; only the first realm's is used.
                let own = self.heap.alloc_object(Object::new(Class::Ordinary, None))?;
                registry.unwrap_or(own)
            },
            table_iterator_prototypes: [object_prototype; 2],
            iterator_helper_prototypes: [object_prototype; 2],
            iterator: object_prototype,
            eval: object_prototype,
            generator_function_prototype: object_prototype,
            generator_prototype: object_prototype,
            promise: object_prototype,
            promise_prototype: object_prototype,
            aggregate_error_prototype: object_prototype,
            async_function_prototype: object_prototype,
            async_generator_function_prototype: object_prototype,
            async_generator_prototype: object_prototype,
            async_from_sync_iterator_prototype: object_prototype,
            regexp: object_prototype,
            regexp_prototype: object_prototype,
            regexp_string_iterator_prototype: object_prototype,
            throw_type_error: object_prototype,
            array_buffer: object_prototype,
            array_buffer_prototype: object_prototype,
            data_view_prototype: object_prototype,
            shared_array_buffer: object_prototype,
            shared_array_buffer_prototype: object_prototype,
            typed_array: object_prototype,
            typed_array_prototype: object_prototype,
            typed_array_constructors: [object_prototype; 12],
            typed_array_prototypes: [object_prototype; 12],
            locale_prototype: object_prototype,
            list_format_prototype: object_prototype,
            collator_prototype: object_prototype,
            plural_rules_prototype: object_prototype,
            intl: [object_prototype; 16],
        };
        // The built-in globals exceed the shape limit in every realm.
        // Start with their final storage and avoid charging transitional shapes.
        self.make_dictionary(global);
        super::builtins::install(self)
    }

    // ---- creation ---------------------------------------------------

    /// `OrdinaryObjectCreate(proto)`.
    pub fn new_object_with(&mut self, proto: Option<ObjectId>) -> JsResult<ObjectId> {
        Ok(self
            .heap
            .alloc_object(Object::new(Class::Ordinary, proto))?)
    }

    /// `OrdinaryObjectCreate(%Object.prototype%)`.
    pub fn new_object(&mut self) -> JsResult<ObjectId> {
        self.new_object_with(Some(self.intrinsics.object_prototype))
    }

    /// `ArrayCreate(0)`.
    pub fn new_array(&mut self) -> JsResult<ObjectId> {
        Ok(self.heap.alloc_object(Object::new(
            Class::Array,
            Some(self.intrinsics.array_prototype),
        ))?)
    }

    /// A string value from Rust text.
    pub fn string(&mut self, text: &str) -> JsResult<Value> {
        Ok(Value::string(self.heap.alloc_str(text)?))
    }

    /// A string value from contents.
    pub fn string_from(&mut self, s: JsStr) -> JsResult<Value> {
        Ok(Value::string(self.heap.alloc_string(s)?))
    }

    /// `CreateBuiltinFunction` with `length` and `name` properties.
    pub fn new_native_function(
        &mut self,
        behavior: NativeFn,
        name: &str,
        length: u32,
    ) -> JsResult<ObjectId> {
        let mut object = Object::new(Class::Function, Some(self.intrinsics.function_prototype));
        object.extra_mut().native = Some(behavior);
        let f = self.heap.alloc_object(object)?;
        let length_key = PropertyKey::String(self.names.length);
        self.define_property_or_throw(
            f,
            length_key,
            PropertyDescriptor::data_with(Value::number(f64::from(length)), false, false, true),
        )?;
        // Names are atoms: most are property keys anyway, so they cost
        // nothing extra.
        let name_value = Value::string(self.heap.intern_str(name)?);
        let name_key = PropertyKey::String(self.names.name);
        self.define_property_or_throw(
            f,
            name_key,
            PropertyDescriptor::data_with(name_value, false, false, true),
        )?;
        Ok(f)
    }

    /// Define a writable, non-enumerable, configurable method on `object`.
    pub fn define_method(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        behavior: NativeFn,
        name: &str,
        length: u32,
    ) -> JsResult<()> {
        let f = self.new_native_function(behavior, name, length)?;
        self.define_property_or_throw(
            object,
            key,
            PropertyDescriptor::data_with(Value::object(f), true, false, true),
        )
    }

    /// A new error object of `kind` with an own `message`.
    pub fn new_error(&mut self, kind: ErrorKind, message: &str) -> JsResult<Value> {
        let proto = match kind {
            ErrorKind::Error => self.intrinsics.error_prototype,
            ErrorKind::TypeError => self.intrinsics.type_error_prototype,
            ErrorKind::RangeError => self.intrinsics.range_error_prototype,
            ErrorKind::ReferenceError => self.intrinsics.reference_error_prototype,
            ErrorKind::SyntaxError => self.intrinsics.syntax_error_prototype,
        };
        let error = self.new_error_object_with(proto)?;
        let message = self.string(message)?;
        let key = PropertyKey::String(self.names.message);
        self.define_property_or_throw(
            error,
            key,
            PropertyDescriptor::data_with(message, true, false, true),
        )?;
        Ok(Value::object(error))
    }

    /// An abrupt completion throwing a new error; out of memory if even
    /// the error cannot be allocated.
    pub fn throw(&mut self, kind: ErrorKind, message: &str) -> Abrupt {
        match self.new_error(kind, message) {
            Ok(error) => Abrupt::Throw(error),
            Err(abrupt) => abrupt,
        }
    }

    /// `Err` of a new TypeError.
    pub fn type_error<T>(&mut self, message: &str) -> JsResult<T> {
        Err(self.throw(ErrorKind::TypeError, message))
    }

    // ---- functions --------------------------------------------------

    /// `IsCallable`.
    pub fn is_callable(&self, value: Value) -> bool {
        value
            .as_object()
            .is_some_and(|o| self.heap.object(o).class == Class::Function)
    }

    /// `Call(f, this, args)`.
    pub fn call(&mut self, f: Value, this: Value, args: &[Value]) -> JsResult<Value> {
        let Some(o) = f.as_object() else {
            return self.type_error("value is not a function");
        };
        if let Some(behavior) = self.heap.object(o).native() {
            return self.call_native(behavior, o, this, args);
        }
        if self.heap.object(o).closure_index().is_some() {
            return self.call_closure(o, this, args);
        }
        self.type_error("value is not a function")
    }

    // ---- inspection -------------------------------------------------

    /// The object's class.
    pub fn class(&self, object: ObjectId) -> Class {
        self.heap.object(object).class
    }

    /// A wrapper object's primitive (`[[BooleanData]]` and so on).
    pub fn primitive_of(&self, object: ObjectId) -> Option<Value> {
        self.heap.object(object).primitive()
    }

    /// Debug text for a value (strings unquoted).
    pub fn display(&self, value: Value) -> String {
        match value.kind() {
            Kind::String(s) => self.heap.string(s).to_text(),
            Kind::Number(n) => number_to_string(n),
            Kind::BigInt(b) => format!("{}n", self.heap.bigint(b).to_string_radix10()),
            Kind::Symbol(s) => {
                let description = self.heap.symbol(s).description;
                format!(
                    "Symbol({})",
                    description.map_or(String::new(), |d| self.heap.string(d).to_text())
                )
            }
            _ => format!("{value:?}"),
        }
    }
}
