//! The agent: one heap, its well-known names and symbols, and the
//! intrinsic objects of its realm (§9.3, §9.7). Built-in library objects
//! beyond what JS-02's conversions need arrive with JS-05.

use super::heap::{DEFAULT_HEAP_BYTES, Heap, OutOfMemory};
use super::number::number_to_string;
use super::object::{Class, Object, PropertyDescriptor};
use super::shape::PropertyKey;
use super::string::JsStr;
use super::value::{Kind, ObjectId, StringId, SymbolId, Value};

/// Why an operation did not complete normally.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Abrupt {
    /// A `throw` completion with the thrown value.
    Throw(Value),
    /// The heap budget ran out. Not catchable by script.
    OutOfMemory,
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
}

impl Intrinsics {
    /// Every intrinsic, for the collector's roots.
    pub(crate) fn all(&self) -> [ObjectId; 14] {
        [
            self.object_prototype,
            self.function_prototype,
            self.array_prototype,
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
        ]
    }
}

/// One agent with one realm.
#[derive(Debug)]
pub struct Agent {
    /// All values of the agent.
    pub heap: Heap,
    /// Interned names.
    pub names: Names,
    /// Well-known symbols.
    pub symbols: WellKnownSymbols,
    /// Intrinsic objects.
    pub intrinsics: Intrinsics,
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
        let mut agent = Agent {
            heap,
            names,
            symbols,
            intrinsics: Intrinsics {
                object_prototype: placeholder,
                function_prototype: placeholder,
                array_prototype: placeholder,
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
            },
        };
        agent.create_intrinsics().map_err(|_| OutOfMemory)?;
        Ok(agent)
    }

    fn create_intrinsics(&mut self) -> JsResult<()> {
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
        };
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
        let name_value = self.string(name)?;
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
        let error = self
            .heap
            .alloc_object(Object::new(Class::Error, Some(proto)))?;
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
        let native = f.as_object().and_then(|o| self.heap.object(o).native());
        match native {
            Some(behavior) => behavior(self, this, args),
            None => self.type_error("value is not a function"),
        }
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
