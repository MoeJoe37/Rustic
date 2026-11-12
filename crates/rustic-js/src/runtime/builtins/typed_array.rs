//! Typed arrays (§23.2) and their exotic internal methods (§10.4.5).
//!
//! A typed array is an object flagged exotic whose internal slot names
//! its buffer, element kind, byte offset and length (`None` for a view
//! that tracks a resizable buffer). It stores no elements of its own:
//! every indexed access goes to the buffer's bytes through
//! `runtime/exotic.rs`.

use std::cmp::Ordering;

use super::buffer::{
    ElementKind, MAX_BUFFER_BYTES, clamp_relative, getter, species_constructor, species_getter,
};
use super::{
    Method, arg, install_methods, iterate, method, new_iterator, prototype_from_new_target,
};
use crate::runtime::agent::{Agent, ErrorKind, JsResult, NativeFn};
use crate::runtime::number::{number_to_string, string_to_number, to_integer_or_infinity};
use crate::runtime::object::{
    Class, FLAG_EXOTIC, FLAG_NATIVE_CONSTRUCTOR, Internal, IteratorKind, Object, PropertyDescriptor,
};
use crate::runtime::shape::PropertyKey;
use crate::runtime::string::JsStr;
use crate::runtime::value::{Kind, ObjectId, Value};

/// A typed array's internal slots.
#[derive(Clone, Copy, Debug)]
pub(crate) struct View {
    /// `[[ViewedArrayBuffer]]`.
    pub buffer: ObjectId,
    /// The element type.
    pub kind: ElementKind,
    /// `[[ByteOffset]]`.
    pub offset: usize,
    /// `[[ArrayLength]]`; `None` tracks the buffer's length.
    pub length: Option<usize>,
}

/// The view record of `o`, if it is a typed array.
pub(crate) fn view_of(agent: &Agent, o: ObjectId) -> Option<View> {
    match agent.heap.object(o).internal() {
        Some(Internal::TypedArray {
            buffer,
            kind,
            offset,
            length,
        }) => Some(View {
            buffer: *buffer,
            kind: *kind,
            offset: *offset,
            length: *length,
        }),
        _ => None,
    }
}

/// `TypedArrayLength`, or `None` when the array is out of bounds or its
/// buffer is detached (`IsTypedArrayOutOfBounds`, §10.4.5.13).
pub(crate) fn length_of(agent: &Agent, view: &View) -> Option<usize> {
    let buffer_length = agent.buffer_len(view.buffer)?;
    if view.offset > buffer_length {
        return None;
    }
    match view.length {
        Some(length) => {
            (view.offset + length * view.kind.size() <= buffer_length).then_some(length)
        }
        None => Some((buffer_length - view.offset) / view.kind.size()),
    }
}

/// `CanonicalNumericIndexString` applied to a property key (§7.1.21):
/// the number a key denotes on a typed array, if it denotes one.
pub(crate) fn numeric_index(agent: &Agent, key: PropertyKey) -> Option<f64> {
    match key {
        PropertyKey::Index(index) => Some(f64::from(index)),
        PropertyKey::Symbol(_) => None,
        PropertyKey::String(s) => {
            let text = agent.heap.string(s);
            // Canonical numeric strings start with a digit, `-`, `I`
            // (Infinity) or `N` (NaN); anything else is an ordinary name.
            let first = text.unit(0)?;
            if !matches!(first, 0x30..=0x39 | 0x2D | 0x49 | 0x4E) {
                return None;
            }
            if *text == JsStr::from_units(&[0x2D, 0x30]) {
                return Some(-0.0);
            }
            let n = string_to_number(text);
            (JsStr::from_units(&number_to_string(n).encode_utf16().collect::<Vec<_>>()) == *text)
                .then_some(n)
        }
    }
}

/// `IsValidIntegerIndex` (§10.4.5.14): the element position of `n`.
fn valid_index(agent: &Agent, view: &View, n: f64) -> Option<usize> {
    let length = length_of(agent, view)?;
    if n.fract() != 0.0 || (n == 0.0 && n.is_sign_negative()) || n < 0.0 || n >= length as f64 {
        return None;
    }
    // An integer below the length.
    Some(n as usize)
}

/// `TypedArrayGetElement` (§10.4.5.15).
fn element_get(agent: &mut Agent, view: &View, n: f64) -> JsResult<Value> {
    let Some(index) = valid_index(agent, view, n) else {
        return Ok(Value::UNDEFINED);
    };
    let size = view.kind.size();
    let bits = agent.buffer_read(view.buffer, view.offset + index * size, size);
    agent.element_value(view.kind, bits)
}

/// `TypedArraySetElement` (§10.4.5.16): the conversion runs first and may
/// run script, so the index is checked after it.
fn element_set(agent: &mut Agent, view: &View, n: f64, value: Value) -> JsResult<()> {
    let bits = agent.element_bits(view.kind, value)?;
    if let Some(index) = valid_index(agent, view, n) {
        let size = view.kind.size();
        agent.buffer_write(view.buffer, view.offset + index * size, size, bits);
    }
    Ok(())
}

// ---- internal methods (§10.4.5.1 to §10.4.5.8) ------------------------------

pub(crate) fn get_own_property(
    agent: &mut Agent,
    o: ObjectId,
    key: PropertyKey,
) -> JsResult<Option<PropertyDescriptor>> {
    let Some(n) = numeric_index(agent, key) else {
        return agent.ordinary_get_own_property(o, key);
    };
    let view = view_of(agent, o).expect("a typed array");
    if valid_index(agent, &view, n).is_none() {
        return Ok(None);
    }
    let value = element_get(agent, &view, n)?;
    Ok(Some(PropertyDescriptor::data(value)))
}

pub(crate) fn has_property(agent: &mut Agent, o: ObjectId, key: PropertyKey) -> JsResult<bool> {
    let Some(n) = numeric_index(agent, key) else {
        return agent.has_property_from(o, key, true);
    };
    let view = view_of(agent, o).expect("a typed array");
    Ok(valid_index(agent, &view, n).is_some())
}

pub(crate) fn define_own_property(
    agent: &mut Agent,
    o: ObjectId,
    key: PropertyKey,
    desc: PropertyDescriptor,
) -> JsResult<bool> {
    let Some(n) = numeric_index(agent, key) else {
        return agent.ordinary_define_own_property(o, key, desc);
    };
    let view = view_of(agent, o).expect("a typed array");
    if valid_index(agent, &view, n).is_none()
        || desc.configurable == Some(false)
        || desc.enumerable == Some(false)
        || desc.is_accessor()
        || desc.writable == Some(false)
    {
        return Ok(false);
    }
    if let Some(value) = desc.value {
        element_set(agent, &view, n, value)?;
    }
    Ok(true)
}

pub(crate) fn get(
    agent: &mut Agent,
    o: ObjectId,
    key: PropertyKey,
    receiver: Value,
) -> JsResult<Value> {
    let Some(n) = numeric_index(agent, key) else {
        return agent.get_from(o, key, receiver, true);
    };
    let view = view_of(agent, o).expect("a typed array");
    element_get(agent, &view, n)
}

pub(crate) fn set(
    agent: &mut Agent,
    o: ObjectId,
    key: PropertyKey,
    value: Value,
    receiver: Value,
) -> JsResult<bool> {
    let Some(n) = numeric_index(agent, key) else {
        return agent.set_from(o, key, value, receiver, true);
    };
    let view = view_of(agent, o).expect("a typed array");
    if receiver == Value::object(o) {
        element_set(agent, &view, n, value)?;
        return Ok(true);
    }
    if valid_index(agent, &view, n).is_none() {
        return Ok(true);
    }
    // `OrdinarySet` with the element as the own writable data property
    // (§10.1.9.2 from step 3): the write lands on the receiver.
    let Some(receiver) = receiver.as_object() else {
        return Ok(false);
    };
    match agent.get_own_property(receiver, key)? {
        Some(existing) => {
            if existing.is_accessor() || existing.writable != Some(true) {
                return Ok(false);
            }
            agent.define_own_property(
                receiver,
                key,
                PropertyDescriptor {
                    value: Some(value),
                    ..Default::default()
                },
            )
        }
        None => agent.create_data_property(receiver, key, value),
    }
}

pub(crate) fn delete(agent: &mut Agent, o: ObjectId, key: PropertyKey) -> JsResult<bool> {
    let Some(n) = numeric_index(agent, key) else {
        return agent.ordinary_delete(o, key);
    };
    let view = view_of(agent, o).expect("a typed array");
    Ok(valid_index(agent, &view, n).is_none())
}

pub(crate) fn own_property_keys(agent: &mut Agent, o: ObjectId) -> Vec<PropertyKey> {
    let view = view_of(agent, o).expect("a typed array");
    let length = length_of(agent, &view).unwrap_or(0);
    // Lengths are bounded by the buffer size limit, below 2³².
    let mut keys: Vec<PropertyKey> = (0..length as u32).map(PropertyKey::Index).collect();
    keys.extend(agent.ordinary_own_property_keys(o));
    keys
}

// ---- creation ------------------------------------------------------------------

fn range_error<T>(agent: &mut Agent, message: &str) -> JsResult<T> {
    Err(agent.throw(ErrorKind::RangeError, message))
}

/// A typed array object over `buffer`.
fn allocate(agent: &mut Agent, prototype: ObjectId, view: View) -> JsResult<ObjectId> {
    agent
        .heap
        .charge(std::mem::size_of::<crate::runtime::Extra>())?;
    let mut object = Object::new(Class::TypedArray, Some(prototype));
    object.set_flag(FLAG_EXOTIC, true);
    object.extra_mut().internal = Some(Internal::TypedArray {
        buffer: view.buffer,
        kind: view.kind,
        offset: view.offset,
        length: view.length,
    });
    Ok(agent.heap.alloc_object(object)?)
}

/// A new typed array of `length` zero elements with its own buffer
/// (`AllocateTypedArray` with a length).
fn allocate_with_length(
    agent: &mut Agent,
    kind: ElementKind,
    prototype: ObjectId,
    length: usize,
) -> JsResult<ObjectId> {
    let Some(bytes) = length
        .checked_mul(kind.size())
        .filter(|b| *b <= MAX_BUFFER_BYTES)
    else {
        return range_error(agent, "Invalid typed array length");
    };
    let buffer_prototype = agent.intrinsics.array_buffer_prototype;
    let buffer = agent.allocate_array_buffer(buffer_prototype, bytes, None)?;
    allocate(
        agent,
        prototype,
        View {
            buffer,
            kind,
            offset: 0,
            length: Some(length),
        },
    )
}

/// `TypedArrayCreateSameType(exemplar, length)` (§23.2.4.3).
fn create_same_type(agent: &mut Agent, kind: ElementKind, length: usize) -> JsResult<ObjectId> {
    let prototype = agent.intrinsics.typed_array_prototypes[kind.index()];
    allocate_with_length(agent, kind, prototype, length)
}

/// `ValidateTypedArray` (§23.2.4.4): the object, its slots and its
/// current length.
fn validate(agent: &mut Agent, value: Value) -> JsResult<(ObjectId, View, usize)> {
    let Some((o, view)) = value
        .as_object()
        .and_then(|o| view_of(agent, o).map(|v| (o, v)))
    else {
        return agent.type_error("the receiver is not a typed array");
    };
    match length_of(agent, &view) {
        Some(length) => Ok((o, view, length)),
        None => agent.type_error("the typed array is detached or out of the bounds of its buffer"),
    }
}

/// `TypedArrayCreateFromCtor(constructor, args)` (§23.2.4.2); `length` is
/// the single numeric argument, when that is what `args` is.
fn create_from_constructor(
    agent: &mut Agent,
    constructor: ObjectId,
    args: &[Value],
    length: Option<usize>,
) -> JsResult<(ObjectId, View)> {
    let created = agent.construct(Value::object(constructor), args)?;
    let (o, view, actual) = validate(agent, created)?;
    if length.is_some_and(|requested| actual < requested) {
        return agent.type_error("the constructor returned a typed array that is too short");
    }
    Ok((o, view))
}

/// `TypedArraySpeciesCreate(exemplar, args)` (§23.2.4.1).
fn species_create(
    agent: &mut Agent,
    exemplar: ObjectId,
    kind: ElementKind,
    args: &[Value],
    length: Option<usize>,
) -> JsResult<(ObjectId, View)> {
    let default = agent.intrinsics.typed_array_constructors[kind.index()];
    let constructor = species_constructor(agent, exemplar, default)?;
    let (o, view) = create_from_constructor(agent, constructor, args, length)?;
    if view.kind.is_bigint() != kind.is_bigint() {
        return agent
            .type_error("the species constructor returned a typed array of another content type");
    }
    Ok((o, view))
}

/// The values an iterable produces (`IteratorToList`).
fn iterable_to_list(agent: &mut Agent, iterable: Value, method: Value) -> JsResult<Vec<Value>> {
    let mut values = Vec::new();
    iterate(agent, iterable, method, |agent, value| {
        agent.heap.charge(8)?;
        values.push(value);
        Ok(true)
    })?;
    Ok(values)
}

/// The constructor of each kind (§23.2.5.1).
fn construct(agent: &mut Agent, kind: ElementKind, args: &[Value]) -> JsResult<Value> {
    if agent.new_target().is_none() {
        return agent.type_error(&format!("Constructor {} requires 'new'", kind.name()));
    }
    let default = agent.intrinsics.typed_array_prototypes[kind.index()];
    let first = arg(args, 0);
    let Some(source) = first.as_object() else {
        let length = agent.to_index(first)?;
        let prototype = prototype_from_new_target(agent, default)?;
        let Ok(length) = usize::try_from(length) else {
            return range_error(agent, "Invalid typed array length");
        };
        return allocate_with_length(agent, kind, prototype, length).map(Value::object);
    };
    let prototype = prototype_from_new_target(agent, default)?;
    if let Some(source_view) = view_of(agent, source) {
        // `InitializeTypedArrayFromTypedArray` (§23.2.5.1.2).
        let Some(length) = length_of(agent, &source_view) else {
            return agent.type_error("the source typed array is detached or out of bounds");
        };
        if source_view.kind.is_bigint() != kind.is_bigint() {
            return agent.type_error("cannot mix BigInt and Number typed arrays");
        }
        let o = allocate_with_length(agent, kind, prototype, length)?;
        let view = view_of(agent, o).expect("just created");
        copy_elements(agent, &source_view, 0, &view, 0, length)?;
        return Ok(Value::object(o));
    }
    if agent.buffer_data(source).is_some() {
        // `InitializeTypedArrayFromArrayBuffer` (§23.2.5.1.3).
        let size = kind.size();
        let offset = agent.to_index(arg(args, 1))?;
        if offset % size as u64 != 0 {
            return range_error(
                agent,
                "the start offset must be a multiple of the element size",
            );
        }
        let requested = arg(args, 2);
        let new_length = if requested == Value::UNDEFINED {
            None
        } else {
            Some(agent.to_index(requested)?)
        };
        let Some(buffer_length) = agent.buffer_len(source) else {
            return agent.type_error("Cannot perform the operation on a detached ArrayBuffer");
        };
        let Some(offset) = usize::try_from(offset).ok().filter(|o| *o <= buffer_length) else {
            return range_error(agent, "the start offset is outside the buffer");
        };
        let resizable = agent.buffer_data(source).is_some_and(|d| d.max.is_some());
        let length = match new_length {
            None if resizable => None,
            None => {
                if buffer_length % size != 0 {
                    return range_error(
                        agent,
                        "the buffer length must be a multiple of the element size",
                    );
                }
                Some((buffer_length - offset) / size)
            }
            Some(length) => {
                let fits = usize::try_from(length)
                    .ok()
                    .and_then(|l| l.checked_mul(size).map(|bytes| (l, bytes)))
                    .filter(|(_, bytes)| *bytes <= buffer_length - offset);
                match fits {
                    Some((length, _)) => Some(length),
                    None => return range_error(agent, "Invalid typed array length"),
                }
            }
        };
        let view = View {
            buffer: source,
            kind,
            offset,
            length,
        };
        return allocate(agent, prototype, view).map(Value::object);
    }
    // An iterable or an array-like (§23.2.5.1 step 6.b.iv).
    let iterator_key = PropertyKey::Symbol(agent.symbols.iterator);
    let values = match agent.get_method(first, iterator_key)? {
        Some(method) => iterable_to_list(agent, first, method)?,
        None => {
            let length = agent.length_of_array_like(source)?;
            if length > MAX_BUFFER_BYTES as f64 {
                return range_error(agent, "Invalid typed array length");
            }
            // Checked against the limit just above.
            let length = length as u32;
            let o = allocate_with_length(agent, kind, prototype, length as usize)?;
            for k in 0..length {
                let value = agent.get(source, PropertyKey::Index(k))?;
                agent.set(o, PropertyKey::Index(k), value, true)?;
            }
            return Ok(Value::object(o));
        }
    };
    let o = allocate_with_length(agent, kind, prototype, values.len())?;
    for (k, value) in values.into_iter().enumerate() {
        // Lengths are below the buffer size limit.
        agent.set(o, PropertyKey::Index(k as u32), value, true)?;
    }
    Ok(Value::object(o))
}

/// Copy `count` elements between two views, converting when their kinds
/// differ. The caller has checked both ranges and the content types.
fn copy_elements(
    agent: &mut Agent,
    source: &View,
    source_at: usize,
    target: &View,
    target_at: usize,
    count: usize,
) -> JsResult<()> {
    let (source_size, target_size) = (source.kind.size(), target.kind.size());
    if source.kind == target.kind {
        let from = source.offset + source_at * source_size;
        let bytes = agent.array_buffer_bytes(source.buffer).expect("attached")
            [from..from + count * source_size]
            .to_vec();
        for (i, chunk) in bytes.chunks(target_size).enumerate() {
            let mut raw = [0u8; 8];
            raw[..target_size].copy_from_slice(chunk);
            agent.buffer_write(
                target.buffer,
                target.offset + (target_at + i) * target_size,
                target_size,
                u64::from_le_bytes(raw),
            );
        }
        return Ok(());
    }
    // Read everything first: the views may share a buffer.
    let mut values = Vec::with_capacity(count);
    for i in 0..count {
        let bits = agent.buffer_read(
            source.buffer,
            source.offset + (source_at + i) * source_size,
            source_size,
        );
        values.push(agent.element_value(source.kind, bits)?);
    }
    for (i, value) in values.into_iter().enumerate() {
        let bits = agent.element_bits(target.kind, value)?;
        agent.buffer_write(
            target.buffer,
            target.offset + (target_at + i) * target_size,
            target_size,
            bits,
        );
    }
    Ok(())
}

macro_rules! constructors {
    ($($name:ident => $kind:ident;)*) => {
        $(
            fn $name(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
                construct(agent, ElementKind::$kind, args)
            }
        )*

        /// The constructor behavior of each kind, in `ElementKind::ALL` order.
        const CONSTRUCTORS: [NativeFn; 12] = [$($name,)*];
    };
}

constructors! {
    int8_array => Int8;
    uint8_array => Uint8;
    uint8_clamped_array => Uint8Clamped;
    int16_array => Int16;
    uint16_array => Uint16;
    int32_array => Int32;
    uint32_array => Uint32;
    float16_array => Float16;
    float32_array => Float32;
    float64_array => Float64;
    big_int64_array => BigInt64;
    big_uint64_array => BigUint64;
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let object_prototype = agent.intrinsics.object_prototype;
    let function_prototype = agent.intrinsics.function_prototype;
    // `%TypedArray%` (§23.2.1): abstract, and not a global.
    let prototype = agent.new_object_with(Some(object_prototype))?;
    let intrinsic = agent.new_native_function(abstract_constructor, "TypedArray", 0)?;
    agent
        .heap
        .object_mut(intrinsic)
        .set_flag(FLAG_NATIVE_CONSTRUCTOR, true);
    let prototype_key = PropertyKey::String(agent.names.prototype);
    let constructor_key = PropertyKey::String(agent.names.constructor);
    agent.define_property_or_throw(
        intrinsic,
        prototype_key,
        PropertyDescriptor::data_with(Value::object(prototype), false, false, false),
    )?;
    agent.define_property_or_throw(
        prototype,
        constructor_key,
        PropertyDescriptor::data_with(Value::object(intrinsic), true, false, true),
    )?;
    install_methods(
        agent,
        intrinsic,
        &[method("from", from, 1), method("of", of, 0)],
    )?;
    species_getter(agent, intrinsic)?;
    install_methods(agent, prototype, PROTOTYPE_METHODS)?;
    getter(agent, prototype, "buffer", get_buffer)?;
    getter(agent, prototype, "byteLength", get_byte_length)?;
    getter(agent, prototype, "byteOffset", get_byte_offset)?;
    getter(agent, prototype, "length", get_length)?;
    // `toString` is the same function object as `Array.prototype.toString`.
    let to_string = PropertyKey::String(agent.names.to_string);
    let array_to_string = agent.get(agent.intrinsics.array_prototype, to_string)?;
    agent.define_property_or_throw(
        prototype,
        to_string,
        PropertyDescriptor::data_with(array_to_string, true, false, true),
    )?;
    let values_key = agent.heap.key("values")?;
    let values = agent.get(prototype, values_key)?;
    agent.define_property_or_throw(
        prototype,
        PropertyKey::Symbol(agent.symbols.iterator),
        PropertyDescriptor::data_with(values, true, false, true),
    )?;
    let tag = agent.new_native_function(get_to_string_tag, "get [Symbol.toStringTag]", 0)?;
    agent.define_property_or_throw(
        prototype,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::accessor(Value::object(tag), Value::UNDEFINED, false, true),
    )?;
    agent.intrinsics.typed_array = intrinsic;
    agent.intrinsics.typed_array_prototype = prototype;

    let global = agent.intrinsics.global;
    let bytes_key = agent.heap.key("BYTES_PER_ELEMENT")?;
    for kind in ElementKind::ALL {
        let name = kind.name();
        let kind_prototype = agent.new_object_with(Some(prototype))?;
        let constructor = agent.new_native_function(CONSTRUCTORS[kind.index()], name, 3)?;
        let object = agent.heap.object_mut(constructor);
        object.set_flag(FLAG_NATIVE_CONSTRUCTOR, true);
        debug_assert_eq!(object.proto(), Some(function_prototype));
        agent.set_prototype_of(constructor, Some(intrinsic))?;
        agent.define_property_or_throw(
            constructor,
            prototype_key,
            PropertyDescriptor::data_with(Value::object(kind_prototype), false, false, false),
        )?;
        agent.define_property_or_throw(
            kind_prototype,
            constructor_key,
            PropertyDescriptor::data_with(Value::object(constructor), true, false, true),
        )?;
        let size = Value::number(kind.size() as f64);
        for target in [constructor, kind_prototype] {
            agent.define_property_or_throw(
                target,
                bytes_key,
                PropertyDescriptor::data_with(size, false, false, false),
            )?;
        }
        let name_key = agent.heap.key(name)?;
        agent.define_property_or_throw(
            global,
            name_key,
            PropertyDescriptor::data_with(Value::object(constructor), true, false, true),
        )?;
        agent.intrinsics.typed_array_prototypes[kind.index()] = kind_prototype;
        agent.intrinsics.typed_array_constructors[kind.index()] = constructor;
    }
    Ok(())
}

/// `%TypedArray%()` (§23.2.1.1) always throws.
fn abstract_constructor(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    agent.type_error("Abstract class TypedArray not directly constructable")
}

/// The receiver as a constructor.
fn this_constructor(agent: &mut Agent, this: Value) -> JsResult<ObjectId> {
    match this
        .as_object()
        .filter(|_| agent.is_constructor_value(this))
    {
        Some(c) => Ok(c),
        None => agent.type_error("the receiver is not a constructor"),
    }
}

/// `%TypedArray%.from(source, mapfn, thisArg)` (§23.2.2.1).
fn from(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let constructor = this_constructor(agent, this)?;
    let map = arg(args, 1);
    if map != Value::UNDEFINED && !agent.is_callable(map) {
        return agent.type_error("%TypedArray%.from: the mapper is not a function");
    }
    let this_arg = arg(args, 2);
    let source = arg(args, 0);
    let iterator_key = PropertyKey::Symbol(agent.symbols.iterator);
    let values = match agent.get_method(source, iterator_key)? {
        Some(method) => iterable_to_list(agent, source, method)?,
        None => {
            let array_like = agent.to_object(source)?;
            let length = agent.length_of_array_like(array_like)?;
            if length > MAX_BUFFER_BYTES as f64 {
                return range_error(agent, "Invalid typed array length");
            }
            // Checked against the limit just above.
            let length = length as u32;
            let (target, _) = create_from_constructor(
                agent,
                constructor,
                &[Value::number(f64::from(length))],
                Some(length as usize),
            )?;
            for k in 0..length {
                let mut value = agent.get(array_like, PropertyKey::Index(k))?;
                if map != Value::UNDEFINED {
                    value = agent.call(map, this_arg, &[value, Value::number(f64::from(k))])?;
                }
                agent.set(target, PropertyKey::Index(k), value, true)?;
            }
            return Ok(Value::object(target));
        }
    };
    let length = values.len();
    let (target, _) = create_from_constructor(
        agent,
        constructor,
        &[Value::number(length as f64)],
        Some(length),
    )?;
    for (k, mut value) in values.into_iter().enumerate() {
        if map != Value::UNDEFINED {
            value = agent.call(map, this_arg, &[value, Value::number(k as f64)])?;
        }
        // Lengths are below the buffer size limit.
        agent.set(target, PropertyKey::Index(k as u32), value, true)?;
    }
    Ok(Value::object(target))
}

/// `%TypedArray%.of(...items)` (§23.2.2.2).
fn of(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let constructor = this_constructor(agent, this)?;
    let (target, _) = create_from_constructor(
        agent,
        constructor,
        &[Value::number(args.len() as f64)],
        Some(args.len()),
    )?;
    for (k, &value) in args.iter().enumerate() {
        // Argument counts are bounded by the value stack limit.
        agent.set(target, PropertyKey::Index(k as u32), value, true)?;
    }
    Ok(Value::object(target))
}

// ---- accessors -----------------------------------------------------------------

/// The receiver's slots, without the bounds check.
fn this_view(agent: &mut Agent, this: Value) -> JsResult<(ObjectId, View)> {
    match this
        .as_object()
        .and_then(|o| view_of(agent, o).map(|v| (o, v)))
    {
        Some(found) => Ok(found),
        None => agent.type_error("the receiver is not a typed array"),
    }
}

fn get_buffer(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, view) = this_view(agent, this)?;
    Ok(Value::object(view.buffer))
}

fn get_byte_length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, view) = this_view(agent, this)?;
    let length = length_of(agent, &view).unwrap_or(0);
    Ok(Value::number((length * view.kind.size()) as f64))
}

fn get_byte_offset(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, view) = this_view(agent, this)?;
    let offset = length_of(agent, &view).map_or(0, |_| view.offset);
    Ok(Value::number(offset as f64))
}

fn get_length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, view) = this_view(agent, this)?;
    Ok(Value::number(length_of(agent, &view).unwrap_or(0) as f64))
}

fn get_to_string_tag(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    match this.as_object().and_then(|o| view_of(agent, o)) {
        Some(view) => agent.string(view.kind.name()),
        None => Ok(Value::UNDEFINED),
    }
}

// ---- methods (§23.2.3) -----------------------------------------------------------

fn index_key(k: usize) -> PropertyKey {
    // Typed array lengths are below the buffer size limit, under 2³².
    PropertyKey::Index(k as u32)
}

fn callback(agent: &mut Agent, args: &[Value], name: &str) -> JsResult<Value> {
    let f = arg(args, 0);
    if agent.is_callable(f) {
        Ok(f)
    } else {
        agent.type_error(&format!(
            "%TypedArray%.prototype.{name}: the callback is not a function"
        ))
    }
}

fn at(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, _, length) = validate(agent, this)?;
    let n = agent.to_number(arg(args, 0))?;
    let relative = to_integer_or_infinity(n);
    let k = if relative >= 0.0 {
        relative
    } else {
        length as f64 + relative
    };
    if k < 0.0 || k >= length as f64 {
        return Ok(Value::UNDEFINED);
    }
    // Between 0 and the length.
    agent.get(o, index_key(k as usize))
}

fn copy_within(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, view, length) = validate(agent, this)?;
    let to = clamp_relative(agent, arg(args, 0), length, 0)?;
    let from = clamp_relative(agent, arg(args, 1), length, 0)?;
    let end = clamp_relative(agent, arg(args, 2), length, length)?;
    let count = end.saturating_sub(from).min(length - to);
    if count > 0 {
        // The arguments ran script: the array may have shrunk.
        let Some(length) = length_of(agent, &view) else {
            return agent
                .type_error("the typed array is detached or out of the bounds of its buffer");
        };
        let size = view.kind.size();
        let limit = view.offset + length * size;
        let to_byte = view.offset + to * size;
        let from_byte = view.offset + from * size;
        let bytes = count * size;
        let bytes = bytes
            .min(limit.saturating_sub(to_byte))
            .min(limit.saturating_sub(from_byte));
        if bytes > 0 {
            let buffer = view.buffer;
            if let Some(Internal::ArrayBuffer(data)) = agent.heap.object_mut(buffer).internal_mut()
            {
                if let Some(data) = data.bytes.as_mut() {
                    data.copy_within(from_byte..from_byte + bytes, to_byte);
                }
            }
        }
    }
    Ok(this)
}

fn entries(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    validate(agent, this)?;
    new_iterator(agent, this, IteratorKind::ArrayEntries)
}

fn keys(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    validate(agent, this)?;
    new_iterator(agent, this, IteratorKind::ArrayKeys)
}

fn values(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    validate(agent, this)?;
    new_iterator(agent, this, IteratorKind::Array)
}

/// Call `f(value, index, array)` for each index until `stop` says so;
/// returns the index and value it stopped at.
fn scan(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
    name: &str,
    backwards: bool,
    stop: fn(bool) -> bool,
) -> JsResult<Option<(usize, Value)>> {
    let (o, _, length) = validate(agent, this)?;
    let f = callback(agent, args, name)?;
    let this_arg = arg(args, 1);
    for step in 0..length {
        let k = if backwards { length - 1 - step } else { step };
        let value = agent.get(o, index_key(k))?;
        let result = agent.call(f, this_arg, &[value, Value::number(k as f64), this])?;
        if stop(agent.to_boolean(result)) {
            return Ok(Some((k, value)));
        }
    }
    Ok(None)
}

fn every(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(
        scan(agent, this, args, "every", false, |r| !r)?.is_none(),
    ))
}

fn some(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(
        scan(agent, this, args, "some", false, |r| r)?.is_some(),
    ))
}

fn for_each(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    scan(agent, this, args, "forEach", false, |_| false)?;
    Ok(Value::UNDEFINED)
}

fn find(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(scan(agent, this, args, "find", false, |r| r)?.map_or(Value::UNDEFINED, |(_, v)| v))
}

fn find_index(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::number(
        scan(agent, this, args, "findIndex", false, |r| r)?.map_or(-1.0, |(k, _)| k as f64),
    ))
}

fn find_last(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(scan(agent, this, args, "findLast", true, |r| r)?.map_or(Value::UNDEFINED, |(_, v)| v))
}

fn find_last_index(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::number(
        scan(agent, this, args, "findLastIndex", true, |r| r)?.map_or(-1.0, |(k, _)| k as f64),
    ))
}

fn fill(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, view, length) = validate(agent, this)?;
    let bits = agent.element_bits(view.kind, arg(args, 0))?;
    let start = clamp_relative(agent, arg(args, 1), length, 0)?;
    let end = clamp_relative(agent, arg(args, 2), length, length)?;
    let Some(length) = length_of(agent, &view) else {
        return agent.type_error("the typed array is detached or out of the bounds of its buffer");
    };
    let size = view.kind.size();
    for k in start..end.min(length) {
        agent.buffer_write(view.buffer, view.offset + k * size, size, bits);
    }
    Ok(this)
}

fn filter(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, view, length) = validate(agent, this)?;
    let f = callback(agent, args, "filter")?;
    let this_arg = arg(args, 1);
    let mut kept = Vec::new();
    for k in 0..length {
        let value = agent.get(o, index_key(k))?;
        let selected = agent.call(f, this_arg, &[value, Value::number(k as f64), this])?;
        if agent.to_boolean(selected) {
            agent.heap.charge(8)?;
            kept.push(value);
        }
    }
    let (target, _) = species_create(
        agent,
        o,
        view.kind,
        &[Value::number(kept.len() as f64)],
        Some(kept.len()),
    )?;
    for (n, value) in kept.into_iter().enumerate() {
        agent.set(target, index_key(n), value, true)?;
    }
    Ok(Value::object(target))
}

fn map(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, view, length) = validate(agent, this)?;
    let f = callback(agent, args, "map")?;
    let this_arg = arg(args, 1);
    let (target, _) = species_create(
        agent,
        o,
        view.kind,
        &[Value::number(length as f64)],
        Some(length),
    )?;
    for k in 0..length {
        let value = agent.get(o, index_key(k))?;
        let mapped = agent.call(f, this_arg, &[value, Value::number(k as f64), this])?;
        agent.set(target, index_key(k), mapped, true)?;
    }
    Ok(Value::object(target))
}

fn includes(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, _, length) = validate(agent, this)?;
    if length == 0 {
        return Ok(Value::FALSE);
    }
    let n = agent.to_number(arg(args, 1))?;
    let n = to_integer_or_infinity(n);
    if n == f64::INFINITY {
        return Ok(Value::FALSE);
    }
    let start = if n >= 0.0 {
        n
    } else {
        (length as f64 + n).max(0.0)
    };
    // `start` is a non-negative integer; past the end the loop is empty.
    for k in (start.min(length as f64) as usize)..length {
        let element = agent.get(o, index_key(k))?;
        if agent.same_value_zero(element, arg(args, 0)) {
            return Ok(Value::TRUE);
        }
    }
    Ok(Value::FALSE)
}

fn index_of(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, _, length) = validate(agent, this)?;
    if length == 0 {
        return Ok(Value::number(-1.0));
    }
    let n = agent.to_number(arg(args, 1))?;
    let n = to_integer_or_infinity(n);
    if n == f64::INFINITY {
        return Ok(Value::number(-1.0));
    }
    let start = if n >= 0.0 {
        n
    } else {
        (length as f64 + n).max(0.0)
    };
    for k in (start.min(length as f64) as usize)..length {
        if agent.has_property(o, index_key(k))? {
            let element = agent.get(o, index_key(k))?;
            if agent.is_strictly_equal(element, arg(args, 0)) {
                return Ok(Value::number(k as f64));
            }
        }
    }
    Ok(Value::number(-1.0))
}

fn last_index_of(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, _, length) = validate(agent, this)?;
    if length == 0 {
        return Ok(Value::number(-1.0));
    }
    let n = if args.len() > 1 {
        let n = agent.to_number(args[1])?;
        to_integer_or_infinity(n)
    } else {
        length as f64 - 1.0
    };
    if n == f64::NEG_INFINITY {
        return Ok(Value::number(-1.0));
    }
    let start = if n >= 0.0 {
        n.min(length as f64 - 1.0)
    } else {
        length as f64 + n
    };
    if start < 0.0 {
        return Ok(Value::number(-1.0));
    }
    // An integer between 0 and the length.
    for k in (0..=start as usize).rev() {
        if agent.has_property(o, index_key(k))? {
            let element = agent.get(o, index_key(k))?;
            if agent.is_strictly_equal(element, arg(args, 0)) {
                return Ok(Value::number(k as f64));
            }
        }
    }
    Ok(Value::number(-1.0))
}

/// The elements joined by `separator`; `text` renders one element.
fn join_elements(
    agent: &mut Agent,
    o: ObjectId,
    length: usize,
    separator: &[u16],
    text: fn(&mut Agent, Value) -> JsResult<Vec<u16>>,
) -> JsResult<Value> {
    let mut out: Vec<u16> = Vec::new();
    for k in 0..length {
        if k > 0 {
            out.extend_from_slice(separator);
        }
        let element = agent.get(o, index_key(k))?;
        if element != Value::UNDEFINED {
            out.extend(text(agent, element)?);
        }
        if out.len() > crate::source::MAX_SOURCE_UNITS {
            return range_error(agent, "Invalid string length");
        }
    }
    agent.string_from(JsStr::from_units(&out))
}

fn join(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, _, length) = validate(agent, this)?;
    let separator = arg(args, 0);
    let separator: Vec<u16> = if separator == Value::UNDEFINED {
        vec![0x2C]
    } else {
        let s = agent.to_string(separator)?;
        agent.heap.string(s).units().collect()
    };
    join_elements(agent, o, length, &separator, |agent, value| {
        let s = agent.to_string(value)?;
        Ok(agent.heap.string(s).units().collect())
    })
}

fn to_locale_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (o, _, length) = validate(agent, this)?;
    join_elements(agent, o, length, &[0x2C], |agent, value| {
        let key = agent.heap.key("toLocaleString")?;
        let text = agent.invoke(value, key, &[])?;
        let s = agent.to_string(text)?;
        Ok(agent.heap.string(s).units().collect())
    })
}

fn reduce_impl(agent: &mut Agent, this: Value, args: &[Value], right: bool) -> JsResult<Value> {
    let (o, _, length) = validate(agent, this)?;
    let name = if right { "reduceRight" } else { "reduce" };
    let f = callback(agent, args, name)?;
    let mut steps = 0..length;
    let position = |step: usize| if right { length - 1 - step } else { step };
    let mut accumulator = match args.get(1) {
        Some(&initial) => initial,
        None => match steps.next() {
            Some(step) => agent.get(o, index_key(position(step)))?,
            None => {
                return agent.type_error("Reduce of empty array with no initial value");
            }
        },
    };
    for step in steps {
        let k = position(step);
        let value = agent.get(o, index_key(k))?;
        accumulator = agent.call(
            f,
            Value::UNDEFINED,
            &[accumulator, value, Value::number(k as f64), this],
        )?;
    }
    Ok(accumulator)
}

fn reduce(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    reduce_impl(agent, this, args, false)
}

fn reduce_right(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    reduce_impl(agent, this, args, true)
}

fn reverse(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, view, length) = validate(agent, this)?;
    let size = view.kind.size();
    let at = |k: usize| view.offset + k * size;
    for lower in 0..length / 2 {
        let upper = length - 1 - lower;
        let a = agent.buffer_read(view.buffer, at(lower), size);
        let b = agent.buffer_read(view.buffer, at(upper), size);
        agent.buffer_write(view.buffer, at(lower), size, b);
        agent.buffer_write(view.buffer, at(upper), size, a);
    }
    Ok(this)
}

/// `%TypedArray%.prototype.set(source, offset)` (§23.2.3.26).
fn set_method(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (target, view) = this_view(agent, this)?;
    let n = agent.to_number(arg(args, 1))?;
    let offset = to_integer_or_infinity(n);
    if offset < 0.0 {
        return range_error(agent, "%TypedArray%.prototype.set: the offset is negative");
    }
    let Some(target_length) = length_of(agent, &view) else {
        return agent.type_error("the typed array is detached or out of the bounds of its buffer");
    };
    let source = arg(args, 0);
    if let Some(source_view) = source.as_object().and_then(|o| view_of(agent, o)) {
        // `SetTypedArrayFromTypedArray` (§23.2.3.26.2).
        let Some(source_length) = length_of(agent, &source_view) else {
            return agent.type_error("the source typed array is detached or out of bounds");
        };
        if source_view.kind.is_bigint() != view.kind.is_bigint() {
            return agent.type_error("cannot mix BigInt and Number typed arrays");
        }
        if offset == f64::INFINITY || source_length as f64 + offset > target_length as f64 {
            return range_error(agent, "%TypedArray%.prototype.set: the source is too large");
        }
        // Checked to be at most the target's length.
        copy_elements(
            agent,
            &source_view,
            0,
            &view,
            offset as usize,
            source_length,
        )?;
        return Ok(Value::UNDEFINED);
    }
    // `SetTypedArrayFromArrayLike` (§23.2.3.26.1).
    let source = agent.to_object(source)?;
    let source_length = agent.length_of_array_like(source)?;
    if offset == f64::INFINITY || source_length + offset > target_length as f64 {
        return range_error(agent, "%TypedArray%.prototype.set: the source is too large");
    }
    // Both are at most the target's length.
    let (offset, source_length) = (offset as usize, source_length as usize);
    for k in 0..source_length {
        let value = agent.get(source, index_key(k))?;
        element_set(agent, &view, (offset + k) as f64, value)?;
    }
    let _ = target;
    Ok(Value::UNDEFINED)
}

fn slice(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, view, length) = validate(agent, this)?;
    let start = clamp_relative(agent, arg(args, 0), length, 0)?;
    let end = clamp_relative(agent, arg(args, 1), length, length)?;
    let count = end.saturating_sub(start);
    let (target, target_view) = species_create(
        agent,
        o,
        view.kind,
        &[Value::number(count as f64)],
        Some(count),
    )?;
    if count > 0 {
        let Some(length) = length_of(agent, &view) else {
            return agent
                .type_error("the typed array is detached or out of the bounds of its buffer");
        };
        let end = end.min(length);
        let count = end.saturating_sub(start);
        if view.kind == target_view.kind {
            // Bytes are copied one by one from the low address, which is
            // what makes a slice into an overlapping view repeat data.
            let size = view.kind.size();
            let target_length = length_of(agent, &target_view).unwrap_or(0);
            let bytes = (count * size).min(target_length * size);
            for i in 0..bytes {
                let byte = agent.buffer_read(view.buffer, view.offset + start * size + i, 1);
                agent.buffer_write(target_view.buffer, target_view.offset + i, 1, byte);
            }
        } else {
            for (n, k) in (start..end).enumerate() {
                let value = agent.get(o, index_key(k))?;
                agent.set(target, index_key(n), value, true)?;
            }
        }
    }
    Ok(Value::object(target))
}

/// The comparator argument of `sort` and `toSorted`.
fn comparator(agent: &mut Agent, args: &[Value], name: &str) -> JsResult<Option<Value>> {
    match arg(args, 0) {
        f if f == Value::UNDEFINED => Ok(None),
        f if agent.is_callable(f) => Ok(Some(f)),
        _ => agent.type_error(&format!(
            "%TypedArray%.prototype.{name}: the comparator must be a function or undefined"
        )),
    }
}

/// The default order (§23.2.4.7): numeric, `-0` before `+0`, NaN last.
fn default_order(agent: &Agent, x: Value, y: Value) -> Ordering {
    match (x.kind(), y.kind()) {
        (Kind::BigInt(a), Kind::BigInt(b)) => agent.heap.bigint(a).cmp(agent.heap.bigint(b)),
        _ => {
            let (a, b) = (
                x.as_number().unwrap_or(f64::NAN),
                y.as_number().unwrap_or(f64::NAN),
            );
            match (a.is_nan(), b.is_nan()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => a
                    .partial_cmp(&b)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| b.is_sign_negative().cmp(&a.is_sign_negative())),
            }
        }
    }
}

/// The elements of `o` in sorted order. The sort is a stable merge sort
/// whose comparison may throw; an inconsistent comparator leaves some
/// order but never loses a value.
fn sorted_values(
    agent: &mut Agent,
    o: ObjectId,
    length: usize,
    compare: Option<Value>,
) -> JsResult<Vec<Value>> {
    agent.heap.charge(16 * length)?;
    let mut items = Vec::with_capacity(length);
    for k in 0..length {
        items.push(agent.get(o, index_key(k))?);
    }
    let Some(f) = compare else {
        items.sort_by(|x, y| default_order(agent, *x, *y));
        return Ok(items);
    };
    let mut width = 1;
    let mut scratch = items.clone();
    while width < length {
        let mut start = 0;
        while start < length {
            let middle = (start + width).min(length);
            let end = (start + 2 * width).min(length);
            let (mut i, mut j, mut out) = (start, middle, start);
            while i < middle && j < end {
                let result = agent.call(f, Value::UNDEFINED, &[items[i], items[j]])?;
                let v = agent.to_number(result)?;
                // Take the right item only when it sorts strictly first.
                if v > 0.0 {
                    scratch[out] = items[j];
                    j += 1;
                } else {
                    scratch[out] = items[i];
                    i += 1;
                }
                out += 1;
            }
            scratch[out..out + (middle - i)].copy_from_slice(&items[i..middle]);
            out += middle - i;
            scratch[out..out + (end - j)].copy_from_slice(&items[j..end]);
            start = end;
        }
        std::mem::swap(&mut items, &mut scratch);
        width *= 2;
    }
    Ok(items)
}

fn sort(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let compare = comparator(agent, args, "sort")?;
    let (o, _, length) = validate(agent, this)?;
    let sorted = sorted_values(agent, o, length, compare)?;
    for (k, value) in sorted.into_iter().enumerate() {
        // A comparator may have shrunk the array; such writes do nothing.
        agent.set(o, index_key(k), value, true)?;
    }
    Ok(this)
}

fn to_sorted(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let compare = comparator(agent, args, "toSorted")?;
    let (o, view, length) = validate(agent, this)?;
    let target = create_same_type(agent, view.kind, length)?;
    let sorted = sorted_values(agent, o, length, compare)?;
    for (k, value) in sorted.into_iter().enumerate() {
        agent.set(target, index_key(k), value, true)?;
    }
    Ok(Value::object(target))
}

fn to_reversed(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (o, view, length) = validate(agent, this)?;
    let target = create_same_type(agent, view.kind, length)?;
    for k in 0..length {
        let value = agent.get(o, index_key(length - 1 - k))?;
        agent.set(target, index_key(k), value, true)?;
    }
    Ok(Value::object(target))
}

fn with(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, view, length) = validate(agent, this)?;
    let n = agent.to_number(arg(args, 0))?;
    let relative = to_integer_or_infinity(n);
    // Adding zero turns the `-0` of a truncated fraction into `+0`.
    let actual = if relative >= 0.0 {
        relative + 0.0
    } else {
        length as f64 + relative
    };
    // The conversion runs before the index check and may shrink the array.
    let value = arg(args, 1);
    let value = if view.kind.is_bigint() {
        let big = agent.to_bigint(value)?;
        agent.bigint_value(big)?
    } else {
        Value::number(agent.to_number(value)?)
    };
    if valid_index(agent, &view, actual).is_none() {
        return range_error(
            agent,
            "%TypedArray%.prototype.with: the index is out of range",
        );
    }
    let target = create_same_type(agent, view.kind, length)?;
    for k in 0..length {
        let element = if k as f64 == actual {
            value
        } else {
            agent.get(o, index_key(k))?
        };
        agent.set(target, index_key(k), element, true)?;
    }
    Ok(Value::object(target))
}

fn subarray(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, view) = this_view(agent, this)?;
    let source_length = length_of(agent, &view).unwrap_or(0);
    let begin = clamp_relative(agent, arg(args, 0), source_length, 0)?;
    let size = view.kind.size();
    let begin_byte = Value::number((view.offset + begin * size) as f64);
    let buffer = Value::object(view.buffer);
    let end = arg(args, 1);
    let (created, _) = if view.length.is_none() && end == Value::UNDEFINED {
        species_create(agent, o, view.kind, &[buffer, begin_byte], None)?
    } else {
        let end = clamp_relative(agent, end, source_length, source_length)?;
        let new_length = Value::number(end.saturating_sub(begin) as f64);
        species_create(agent, o, view.kind, &[buffer, begin_byte, new_length], None)?
    };
    Ok(Value::object(created))
}

const PROTOTYPE_METHODS: &[Method] = &[
    method("at", at, 1),
    method("copyWithin", copy_within, 2),
    method("entries", entries, 0),
    method("every", every, 1),
    method("fill", fill, 1),
    method("filter", filter, 1),
    method("find", find, 1),
    method("findIndex", find_index, 1),
    method("findLast", find_last, 1),
    method("findLastIndex", find_last_index, 1),
    method("forEach", for_each, 1),
    method("includes", includes, 1),
    method("indexOf", index_of, 1),
    method("join", join, 1),
    method("keys", keys, 0),
    method("lastIndexOf", last_index_of, 1),
    method("map", map, 1),
    method("reduce", reduce, 1),
    method("reduceRight", reduce_right, 1),
    method("reverse", reverse, 0),
    method("set", set_method, 1),
    method("slice", slice, 2),
    method("some", some, 1),
    method("sort", sort, 1),
    method("subarray", subarray, 2),
    method("toLocaleString", to_locale_string, 0),
    method("toReversed", to_reversed, 0),
    method("toSorted", to_sorted, 1),
    method("values", values, 0),
    method("with", with, 2),
];
