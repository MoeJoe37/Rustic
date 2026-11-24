//! ArrayBuffer (§25.1) and DataView (§25.3), and the element codec they
//! share with typed arrays (`typed_array.rs`).
//!
//! A buffer's bytes are one `Vec<u8>` charged to the heap budget. Element
//! values are stored little-endian on every target, so a typed array reads
//! the same bytes on a big-endian host (the specification leaves the
//! order to the implementation).

use super::{arg, install_constructor, install_methods, method, prototype_from_new_target};
use crate::runtime::agent::{Agent, ErrorKind, JsResult};
use crate::runtime::number::{to_integer_or_infinity, to_uint32};
use crate::runtime::object::{Class, Internal, Object, PropertyDescriptor};
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{ObjectId, Value};

/// The most bytes one buffer may hold. Indices then fit in the `u32`
/// array-index space and in `usize` on 32-bit targets; the heap budget is
/// usually the tighter limit.
pub(crate) const MAX_BUFFER_BYTES: usize = i32::MAX as usize;

/// `[[ArrayBufferData]]` and friends.
#[derive(Clone, Debug)]
pub(crate) struct BufferData {
    /// The bytes; `None` once detached.
    pub bytes: Option<Vec<u8>>,
    /// `[[ArrayBufferMaxByteLength]]` of a resizable or growable buffer.
    pub max: Option<usize>,
    /// A SharedArrayBuffer: never detached, and it only grows.
    pub shared: bool,
    /// Whether the byte storage can be written or detached.
    pub immutable: bool,
}

/// The element type of a typed array or a DataView accessor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ElementKind {
    Int8,
    Uint8,
    Uint8Clamped,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Float16,
    Float32,
    Float64,
    BigInt64,
    BigUint64,
}

impl ElementKind {
    /// Every kind, in the order of the constructor tables.
    pub(crate) const ALL: [ElementKind; 12] = [
        ElementKind::Int8,
        ElementKind::Uint8,
        ElementKind::Uint8Clamped,
        ElementKind::Int16,
        ElementKind::Uint16,
        ElementKind::Int32,
        ElementKind::Uint32,
        ElementKind::Float16,
        ElementKind::Float32,
        ElementKind::Float64,
        ElementKind::BigInt64,
        ElementKind::BigUint64,
    ];

    /// The element size in bytes.
    pub(crate) fn size(self) -> usize {
        match self {
            ElementKind::Int8 | ElementKind::Uint8 | ElementKind::Uint8Clamped => 1,
            ElementKind::Int16 | ElementKind::Uint16 | ElementKind::Float16 => 2,
            ElementKind::Int32 | ElementKind::Uint32 | ElementKind::Float32 => 4,
            ElementKind::Float64 | ElementKind::BigInt64 | ElementKind::BigUint64 => 8,
        }
    }

    /// Whether elements are BigInts (`[[ContentType]]`).
    pub(crate) fn is_bigint(self) -> bool {
        matches!(self, ElementKind::BigInt64 | ElementKind::BigUint64)
    }

    /// The position in [`ElementKind::ALL`].
    pub(crate) fn index(self) -> usize {
        ElementKind::ALL
            .iter()
            .position(|k| *k == self)
            .expect("every kind is listed")
    }

    /// The constructor's name.
    pub(crate) fn name(self) -> &'static str {
        match self {
            ElementKind::Int8 => "Int8Array",
            ElementKind::Uint8 => "Uint8Array",
            ElementKind::Uint8Clamped => "Uint8ClampedArray",
            ElementKind::Int16 => "Int16Array",
            ElementKind::Uint16 => "Uint16Array",
            ElementKind::Int32 => "Int32Array",
            ElementKind::Uint32 => "Uint32Array",
            ElementKind::Float16 => "Float16Array",
            ElementKind::Float32 => "Float32Array",
            ElementKind::Float64 => "Float64Array",
            ElementKind::BigInt64 => "BigInt64Array",
            ElementKind::BigUint64 => "BigUint64Array",
        }
    }
}

/// The nearest binary16 value to `x`, ties to even, as its bits.
pub(crate) fn f64_to_f16_bits(x: f64) -> u16 {
    if x.is_nan() {
        return 0x7E00;
    }
    let sign = if x.is_sign_negative() { 0x8000 } else { 0 };
    let a = x.abs();
    // 65520 is halfway between the largest finite value and 2¹⁶; the tie
    // goes to the even mantissa, which is infinity's.
    if a >= 65520.0 {
        return sign | 0x7C00;
    }
    // Below the normal range every value is a multiple of 2⁻²⁴.
    if a < 6.103_515_625e-5 {
        let m = (a * 16_777_216.0).round_ties_even();
        // At most 1024, which is the bit pattern of the smallest normal.
        return sign | m as u16;
    }
    let exponent = ((a.to_bits() >> 52) as i32) - 1023;
    let scaled = a / f64::powi(2.0, exponent - 10);
    let mut mantissa = scaled.round_ties_even();
    let mut exponent = exponent;
    if mantissa == 2048.0 {
        mantissa = 1024.0;
        exponent += 1;
    }
    if exponent > 15 {
        return sign | 0x7C00;
    }
    // `exponent` is in -14..=15 and `mantissa` in 1024..2048 here.
    sign | (((exponent + 15) as u16) << 10) | (mantissa as u16 - 1024)
}

/// The value of binary16 bits.
pub(crate) fn f16_bits_to_f64(bits: u16) -> f64 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1F);
    let mantissa = f64::from(bits & 0x3FF);
    let magnitude = match exponent {
        0 => mantissa * f64::powi(2.0, -24),
        31 if mantissa == 0.0 => f64::INFINITY,
        31 => return f64::NAN,
        _ => (1024.0 + mantissa) * f64::powi(2.0, exponent - 25),
    };
    sign * magnitude
}

/// `ToUint8Clamp` (§7.1.12).
fn to_uint8_clamp(n: f64) -> u8 {
    if n.is_nan() {
        return 0;
    }
    // In range after the clamp, and ties go to even.
    n.clamp(0.0, 255.0).round_ties_even() as u8
}

impl Agent {
    /// Convert an IDL `AllowSharedBufferSource` without reading its bytes.
    /// Shared fixed-length buffers are accepted. Resizable or growable
    /// buffers require a different IDL type.
    pub fn web_buffer_source(&mut self, value: Value) -> JsResult<ObjectId> {
        self.convert_web_buffer(value, true)
    }

    /// Whether a value has a buffer or buffer view internal slot, before
    /// applying any Web IDL restrictions on the backing store.
    pub fn is_web_buffer_source(&self, value: Value) -> bool {
        value
            .as_object()
            .and_then(|object| self.source_buffer(object))
            .is_some()
    }

    /// Convert an IDL `[AllowResizable] AllowSharedBufferSource`.
    pub fn web_resizable_buffer_source(&mut self, value: Value) -> JsResult<ObjectId> {
        if let Some(object) = value.as_object() {
            if self.source_buffer(object).is_some() {
                return Ok(object);
            }
        }
        self.type_error("expected a buffer source")
    }

    /// Convert an IDL `BufferSource` whose backing store cannot be shared
    /// or resizable.
    pub fn web_unshared_buffer_source(&mut self, value: Value) -> JsResult<ObjectId> {
        self.convert_web_buffer(value, false)
    }

    fn convert_web_buffer(&mut self, value: Value, allow_shared: bool) -> JsResult<ObjectId> {
        if let Some(object) = value.as_object() {
            if self
                .source_buffer(object)
                .and_then(|buffer| self.buffer_data(buffer))
                .is_some_and(|data| data.max.is_none() && (allow_shared || !data.shared))
            {
                return Ok(object);
            }
        }
        self.type_error("invalid backing store for this buffer source")
    }

    fn source_buffer(&self, object: ObjectId) -> Option<ObjectId> {
        match self.heap.object(object).internal() {
            Some(Internal::ArrayBuffer(_)) => Some(object),
            Some(Internal::TypedArray { buffer, .. } | Internal::DataView { buffer, .. }) => {
                Some(*buffer)
            }
            _ => None,
        }
    }

    /// Convert an IDL `[AllowShared] Uint8Array`. Proxies and other element
    /// kinds do not have the required internal slots.
    pub fn web_uint8_array(&mut self, value: Value) -> JsResult<ObjectId> {
        if value.as_object().is_some_and(|object| {
            super::typed_array::view_of(self, object).is_some_and(|v| v.kind == ElementKind::Uint8)
        }) {
            return self.web_buffer_source(value);
        }
        self.type_error("expected a Uint8Array")
    }

    /// The bytes of a converted buffer source while no script can run.
    /// Detachment after conversion (for example by a dictionary getter)
    /// leaves an empty byte sequence.
    pub fn web_buffer_bytes(&self, source: ObjectId) -> &[u8] {
        let (buffer, offset, length) = match self.heap.object(source).internal() {
            Some(Internal::ArrayBuffer(_)) => (source, 0, None),
            Some(Internal::DataView {
                buffer,
                offset,
                length,
            }) => (*buffer, *offset, *length),
            Some(Internal::TypedArray {
                buffer,
                offset,
                kind,
                length,
            }) => (*buffer, *offset, length.map(|length| length * kind.size())),
            _ => return &[],
        };
        let Some(bytes) = self
            .buffer_data(buffer)
            .and_then(|data| data.bytes.as_deref())
        else {
            return &[];
        };
        match length {
            Some(length) => bytes.get(offset..offset + length),
            None => bytes.get(offset..),
        }
        .unwrap_or_default()
    }

    /// Fill the prefix of a converted Uint8Array. The caller determines
    /// the amount to write without executing script after conversion.
    pub fn write_web_uint8_array(&mut self, destination: ObjectId, bytes: &[u8]) -> JsResult<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let Some(view) = super::typed_array::view_of(self, destination)
            .filter(|view| view.kind == ElementKind::Uint8)
        else {
            return self.type_error("expected a Uint8Array");
        };
        if bytes.len() > self.web_buffer_bytes(destination).len() {
            return self.type_error("destination no longer covers the bytes");
        }
        self.require_mutable_buffer(view.buffer)?;
        let target = self
            .buffer_data_mut(view.buffer)
            .and_then(|data| data.bytes.as_mut())
            .expect("an in-bounds Uint8Array");
        target[view.offset..view.offset + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }

    /// The raw element for `value` (`NumericToRawBytes` after the
    /// conversion of `TypedArraySetElement`): the low bytes of the result
    /// are the element, least significant first.
    pub(crate) fn element_bits(&mut self, kind: ElementKind, value: Value) -> JsResult<u64> {
        if kind.is_bigint() {
            return self.to_biguint64(value);
        }
        let n = self.to_number(value)?;
        Ok(match kind {
            ElementKind::Uint8Clamped => u64::from(to_uint8_clamp(n)),
            ElementKind::Float16 => u64::from(f64_to_f16_bits(n)),
            // Rounding to binary32 is the conversion.
            ElementKind::Float32 => u64::from((n as f32).to_bits()),
            ElementKind::Float64 => n.to_bits(),
            // The integer kinds keep the low bits of the value modulo 2³².
            _ => u64::from(to_uint32(n)),
        })
    }

    /// The value of raw element bits (`RawBytesToNumeric`).
    pub(crate) fn element_value(&mut self, kind: ElementKind, bits: u64) -> JsResult<Value> {
        // Each cast below keeps exactly the element's low bytes.
        Ok(match kind {
            ElementKind::Int8 => Value::number(f64::from(bits as u8 as i8)),
            ElementKind::Uint8 | ElementKind::Uint8Clamped => Value::number(f64::from(bits as u8)),
            ElementKind::Int16 => Value::number(f64::from(bits as u16 as i16)),
            ElementKind::Uint16 => Value::number(f64::from(bits as u16)),
            ElementKind::Int32 => Value::number(f64::from(bits as u32 as i32)),
            ElementKind::Uint32 => Value::number(f64::from(bits as u32)),
            ElementKind::Float16 => Value::number(f16_bits_to_f64(bits as u16)),
            ElementKind::Float32 => Value::number(f64::from(f32::from_bits(bits as u32))),
            ElementKind::Float64 => Value::number(f64::from_bits(bits)),
            ElementKind::BigInt64 => {
                self.bigint_value(crate::runtime::BigInt::from_i64(bits as i64))?
            }
            ElementKind::BigUint64 => self.bigint_value(crate::runtime::BigInt::from_u64(bits))?,
        })
    }

    /// The buffer record of `o`, if it is an ArrayBuffer.
    pub(crate) fn buffer_data(&self, o: ObjectId) -> Option<&BufferData> {
        match self.heap.object(o).internal() {
            Some(Internal::ArrayBuffer(data)) => Some(data),
            _ => None,
        }
    }

    fn buffer_data_mut(&mut self, o: ObjectId) -> Option<&mut BufferData> {
        match self.heap.object_mut(o).internal_mut() {
            Some(Internal::ArrayBuffer(data)) => Some(data),
            _ => None,
        }
    }

    /// The byte length of buffer `o`; `None` when it is detached.
    pub(crate) fn buffer_len(&self, o: ObjectId) -> Option<usize> {
        self.buffer_data(o)?.bytes.as_ref().map(Vec::len)
    }

    /// Read `size` bytes at `at` as little-endian bits. The caller has
    /// checked the range.
    pub(crate) fn buffer_read(&self, o: ObjectId, at: usize, size: usize) -> u64 {
        let bytes = self
            .buffer_data(o)
            .and_then(|d| d.bytes.as_ref())
            .expect("an attached buffer");
        let mut raw = [0u8; 8];
        raw[..size].copy_from_slice(&bytes[at..at + size]);
        u64::from_le_bytes(raw)
    }

    /// Write the low `size` bytes of `bits` at `at`, little-endian. The
    /// caller has checked the range.
    pub(crate) fn buffer_write(
        &mut self,
        o: ObjectId,
        at: usize,
        size: usize,
        bits: u64,
    ) -> JsResult<()> {
        self.require_mutable_buffer(o)?;
        let bytes = self
            .buffer_data_mut(o)
            .and_then(|d| d.bytes.as_mut())
            .expect("an attached buffer");
        bytes[at..at + size].copy_from_slice(&bits.to_le_bytes()[..size]);
        Ok(())
    }

    pub(crate) fn buffer_is_immutable(&self, buffer: ObjectId) -> bool {
        self.buffer_data(buffer).is_some_and(|data| data.immutable)
    }

    pub(crate) fn require_mutable_buffer(&mut self, buffer: ObjectId) -> JsResult<()> {
        if self.buffer_is_immutable(buffer) {
            return self.type_error("ArrayBuffer is immutable");
        }
        Ok(())
    }

    pub(crate) fn immutable_bytes_buffer(&mut self, bytes: &[u8]) -> JsResult<ObjectId> {
        self.array_buffer_from_bytes(bytes, true)
    }

    /// Create a mutable ArrayBuffer in the relevant realm of a Web IDL
    /// receiver and copy bytes into its backing store.
    pub fn new_web_array_buffer(&mut self, owner: ObjectId, bytes: &[u8]) -> JsResult<Value> {
        let previous = self.heap.realm;
        self.switch_realm(self.heap.object(owner).realm);
        let result = self.array_buffer_from_bytes(bytes, false);
        self.switch_realm(previous);
        result.map(Value::object)
    }

    fn array_buffer_from_bytes(&mut self, bytes: &[u8], immutable: bool) -> JsResult<ObjectId> {
        let buffer = self.new_array_buffer(bytes.len())?;
        let data = self.buffer_data_mut(buffer).expect("new array buffer");
        data.bytes
            .as_mut()
            .expect("attached array buffer")
            .copy_from_slice(bytes);
        data.immutable = immutable;
        Ok(buffer)
    }

    /// `AllocateArrayBuffer` (§25.1.3.1): a zeroed buffer, or a RangeError
    /// when the length is over the limit or the heap budget.
    pub(crate) fn allocate_array_buffer(
        &mut self,
        prototype: ObjectId,
        length: usize,
        max: Option<usize>,
    ) -> JsResult<ObjectId> {
        self.allocate_buffer(prototype, length, max, false)
    }

    /// `AllocateSharedArrayBuffer` (§25.2.2.1).
    fn allocate_shared_array_buffer(
        &mut self,
        prototype: ObjectId,
        length: usize,
        max: Option<usize>,
    ) -> JsResult<ObjectId> {
        self.allocate_buffer(prototype, length, max, true)
    }

    fn allocate_buffer(
        &mut self,
        prototype: ObjectId,
        length: usize,
        max: Option<usize>,
        shared: bool,
    ) -> JsResult<ObjectId> {
        if length > MAX_BUFFER_BYTES
            || max.is_some_and(|m| m > MAX_BUFFER_BYTES)
            || self
                .heap
                .charge(length + std::mem::size_of::<crate::runtime::Extra>())
                .is_err()
        {
            return Err(self.throw(ErrorKind::RangeError, "Array buffer allocation failed"));
        }
        let mut object = Object::new(Class::ArrayBuffer, Some(prototype));
        object.extra_mut().internal = Some(Internal::ArrayBuffer(Box::new(BufferData {
            bytes: Some(vec![0; length]),
            max,
            shared,
            immutable: false,
        })));
        Ok(self.heap.alloc_object(object)?)
    }

    /// A new `SharedArrayBuffer` of `length` zero bytes. Its memory is
    /// shared between the typed arrays and views of this agent only:
    /// there is no other agent to share it with yet (TD-115).
    pub fn new_shared_array_buffer(&mut self, length: usize) -> JsResult<ObjectId> {
        let prototype = self.intrinsics.shared_array_buffer_prototype;
        self.allocate_shared_array_buffer(prototype, length, None)
    }

    /// Whether `o` is a SharedArrayBuffer (`IsSharedArrayBuffer`).
    pub fn is_shared_array_buffer(&self, o: ObjectId) -> bool {
        self.buffer_data(o).is_some_and(|data| data.shared)
    }

    /// A new `ArrayBuffer` of `length` zero bytes.
    pub fn new_array_buffer(&mut self, length: usize) -> JsResult<ObjectId> {
        let prototype = self.intrinsics.array_buffer_prototype;
        self.allocate_array_buffer(prototype, length, None)
    }

    /// `DetachArrayBuffer` (§25.1.3.5): hosts detach a buffer they
    /// transfer. Returns whether `buffer` is an ArrayBuffer; a
    /// SharedArrayBuffer cannot be detached.
    pub fn detach_array_buffer(&mut self, buffer: ObjectId) -> bool {
        match self
            .buffer_data_mut(buffer)
            .filter(|data| !data.shared && !data.immutable)
        {
            Some(data) => {
                // A detached resizable buffer still reports `resizable`.
                data.bytes = None;
                true
            }
            None => false,
        }
    }

    /// The bytes of an attached ArrayBuffer.
    pub fn array_buffer_bytes(&self, buffer: ObjectId) -> Option<&[u8]> {
        self.buffer_data(buffer)?.bytes.as_deref()
    }
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let object_prototype = agent.intrinsics.object_prototype;
    let prototype = agent.new_object_with(Some(object_prototype))?;
    agent.intrinsics.array_buffer_prototype = prototype;
    let constructor = install_constructor(agent, "ArrayBuffer", buffer_constructor, 1, prototype)?;
    agent.intrinsics.array_buffer = constructor;
    install_methods(agent, constructor, &[method("isView", buffer_is_view, 1)])?;
    species_getter(agent, constructor)?;
    install_methods(
        agent,
        prototype,
        &[
            method("slice", buffer_slice, 2),
            method("resize", buffer_resize, 1),
            method("transfer", buffer_transfer, 0),
            method("transferToFixedLength", buffer_transfer_fixed, 0),
        ],
    )?;
    getter(agent, prototype, "byteLength", buffer_byte_length)?;
    getter(agent, prototype, "maxByteLength", buffer_max_byte_length)?;
    getter(agent, prototype, "resizable", buffer_resizable)?;
    getter(agent, prototype, "detached", buffer_detached)?;
    getter(agent, prototype, "immutable", buffer_immutable)?;
    to_string_tag(agent, prototype, "ArrayBuffer")?;

    let shared_prototype = agent.new_object_with(Some(object_prototype))?;
    agent.intrinsics.shared_array_buffer_prototype = shared_prototype;
    let shared = install_constructor(
        agent,
        "SharedArrayBuffer",
        shared_constructor,
        1,
        shared_prototype,
    )?;
    agent.intrinsics.shared_array_buffer = shared;
    species_getter(agent, shared)?;
    install_methods(
        agent,
        shared_prototype,
        &[
            method("grow", shared_grow, 1),
            method("slice", shared_slice, 2),
        ],
    )?;
    getter(agent, shared_prototype, "byteLength", shared_byte_length)?;
    getter(agent, shared_prototype, "growable", shared_growable)?;
    getter(
        agent,
        shared_prototype,
        "maxByteLength",
        shared_max_byte_length,
    )?;
    to_string_tag(agent, shared_prototype, "SharedArrayBuffer")?;

    let view_prototype = agent.new_object_with(Some(object_prototype))?;
    agent.intrinsics.data_view_prototype = view_prototype;
    install_constructor(agent, "DataView", data_view_constructor, 1, view_prototype)?;
    install_methods(agent, view_prototype, DATA_VIEW_METHODS)?;
    getter(agent, view_prototype, "buffer", data_view_buffer)?;
    getter(agent, view_prototype, "byteLength", data_view_byte_length)?;
    getter(agent, view_prototype, "byteOffset", data_view_byte_offset)?;
    to_string_tag(agent, view_prototype, "DataView")
}

/// An accessor property `name` with only a getter.
pub(super) fn getter(
    agent: &mut Agent,
    target: ObjectId,
    name: &str,
    behavior: crate::runtime::NativeFn,
) -> JsResult<()> {
    let function = agent.new_native_function(behavior, &format!("get {name}"), 0)?;
    let key = agent.heap.key(name)?;
    agent.define_property_or_throw(
        target,
        key,
        PropertyDescriptor::accessor(Value::object(function), Value::UNDEFINED, false, true),
    )
}

pub(super) fn to_string_tag(agent: &mut Agent, target: ObjectId, tag: &str) -> JsResult<()> {
    let tag = agent.string(tag)?;
    agent.define_property_or_throw(
        target,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )
}

fn return_this(_: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(this)
}

/// `get C[Symbol.species]`, which returns the receiver.
pub(super) fn species_getter(agent: &mut Agent, constructor: ObjectId) -> JsResult<()> {
    let function = agent.new_native_function(return_this, "get [Symbol.species]", 0)?;
    agent.define_property_or_throw(
        constructor,
        PropertyKey::Symbol(agent.symbols.species),
        PropertyDescriptor::accessor(Value::object(function), Value::UNDEFINED, false, true),
    )
}

/// `SpeciesConstructor(O, default)` (§7.3.22).
pub(super) fn species_constructor(
    agent: &mut Agent,
    o: ObjectId,
    default: ObjectId,
) -> JsResult<ObjectId> {
    let c = agent.get(o, PropertyKey::String(agent.names.constructor))?;
    if c == Value::UNDEFINED {
        return Ok(default);
    }
    let Some(c) = c.as_object() else {
        return agent.type_error("the constructor property is not an object");
    };
    let s = agent.get(c, PropertyKey::Symbol(agent.symbols.species))?;
    if s.is_nullish() {
        return Ok(default);
    }
    match s.as_object().filter(|_| agent.is_constructor_value(s)) {
        Some(s) => Ok(s),
        None => agent.type_error("the species is not a constructor"),
    }
}

/// `ToIndex` narrowed to what a buffer can hold; larger is a RangeError.
pub(super) fn to_byte_index(agent: &mut Agent, value: Value) -> JsResult<usize> {
    let index = agent.to_index(value)?;
    match usize::try_from(index) {
        Ok(index) if index <= MAX_BUFFER_BYTES => Ok(index),
        _ => Err(agent.throw(ErrorKind::RangeError, "Array buffer allocation failed")),
    }
}

/// `ArrayBuffer(length, options)` (§25.1.4.1).
fn buffer_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    construct_buffer(agent, args, false)
}

/// `SharedArrayBuffer(length, options)` (§25.2.3.1).
fn shared_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    construct_buffer(agent, args, true)
}

fn construct_buffer(agent: &mut Agent, args: &[Value], shared: bool) -> JsResult<Value> {
    if agent.new_target().is_none() {
        return agent.type_error(if shared {
            "Constructor SharedArrayBuffer requires 'new'"
        } else {
            "Constructor ArrayBuffer requires 'new'"
        });
    }
    let length = agent.to_index(arg(args, 0))?;
    // `GetArrayBufferMaxByteLengthOption`.
    let max = match arg(args, 1).as_object() {
        None => None,
        Some(options) => {
            let key = agent.heap.key("maxByteLength")?;
            let max = agent.get(options, key)?;
            if max == Value::UNDEFINED {
                None
            } else {
                Some(agent.to_index(max)?)
            }
        }
    };
    // Checked before the prototype is read (`AllocateArrayBuffer` step 3).
    if max.is_some_and(|max| length > max) {
        return Err(agent.throw(
            ErrorKind::RangeError,
            "ArrayBuffer length exceeds maxByteLength",
        ));
    }
    let default = if shared {
        agent.intrinsics.shared_array_buffer_prototype
    } else {
        agent.intrinsics.array_buffer_prototype
    };
    let prototype = prototype_from_new_target(agent, default)?;
    let too_big = |n: u64| usize::try_from(n).map_or(true, |n| n > MAX_BUFFER_BYTES);
    if too_big(length) || max.is_some_and(too_big) {
        return Err(agent.throw(ErrorKind::RangeError, "Array buffer allocation failed"));
    }
    // Both were just checked to fit.
    let buffer =
        agent.allocate_buffer(prototype, length as usize, max.map(|m| m as usize), shared)?;
    Ok(Value::object(buffer))
}

/// `ArrayBuffer.isView(arg)` (§25.1.5.1).
fn buffer_is_view(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let is_view = arg(args, 0).as_object().is_some_and(|o| {
        matches!(
            agent.heap.object(o).internal(),
            Some(Internal::TypedArray { .. } | Internal::DataView { .. })
        )
    });
    Ok(Value::boolean(is_view))
}

/// The receiver as an ArrayBuffer (not a SharedArrayBuffer).
fn this_buffer(agent: &mut Agent, this: Value, method: &str) -> JsResult<ObjectId> {
    match this
        .as_object()
        .filter(|o| agent.buffer_data(*o).is_some_and(|data| !data.shared))
    {
        Some(o) => Ok(o),
        None => agent.type_error(&format!(
            "ArrayBuffer.prototype.{method} called on an incompatible receiver"
        )),
    }
}

fn buffer_byte_length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let buffer = this_buffer(agent, this, "byteLength")?;
    Ok(Value::number(agent.buffer_len(buffer).unwrap_or(0) as f64))
}

fn buffer_max_byte_length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let buffer = this_buffer(agent, this, "maxByteLength")?;
    let data = agent.buffer_data(buffer).expect("checked above");
    let length = match (&data.bytes, data.max) {
        (None, _) => 0,
        (Some(_), Some(max)) => max,
        (Some(bytes), None) => bytes.len(),
    };
    Ok(Value::number(length as f64))
}

fn buffer_resizable(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let buffer = this_buffer(agent, this, "resizable")?;
    let data = agent.buffer_data(buffer).expect("checked above");
    Ok(Value::boolean(data.max.is_some()))
}

fn buffer_detached(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let buffer = this_buffer(agent, this, "detached")?;
    Ok(Value::boolean(agent.buffer_len(buffer).is_none()))
}

fn buffer_immutable(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let buffer = this_buffer(agent, this, "immutable")?;
    Ok(Value::boolean(agent.buffer_is_immutable(buffer)))
}

fn detached_error<T>(agent: &mut Agent) -> JsResult<T> {
    agent.type_error("Cannot perform the operation on a detached ArrayBuffer")
}

/// A relative index clamped to `0..=length` (the `start`/`end` arguments
/// of `slice` and friends).
pub(super) fn clamp_relative(
    agent: &mut Agent,
    value: Value,
    length: usize,
    default: usize,
) -> JsResult<usize> {
    if value == Value::UNDEFINED {
        return Ok(default);
    }
    let n = agent.to_number(value)?;
    let relative = to_integer_or_infinity(n);
    let length_f = length as f64;
    let clamped = if relative < 0.0 {
        (length_f + relative).max(0.0)
    } else {
        relative.min(length_f)
    };
    // Between 0 and `length`.
    Ok(clamped as usize)
}

/// `ArrayBuffer.prototype.slice(start, end)` (§25.1.6.7).
fn buffer_slice(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let buffer = this_buffer(agent, this, "slice")?;
    let Some(length) = agent.buffer_len(buffer) else {
        return detached_error(agent);
    };
    let first = clamp_relative(agent, arg(args, 0), length, 0)?;
    let last = clamp_relative(agent, arg(args, 1), length, length)?;
    let new_length = last.saturating_sub(first);
    let default = agent.intrinsics.array_buffer;
    let constructor = species_constructor(agent, buffer, default)?;
    let created = agent.construct(
        Value::object(constructor),
        &[Value::number(new_length as f64)],
    )?;
    let Some(new) = created
        .as_object()
        .filter(|o| agent.buffer_data(*o).is_some_and(|data| !data.shared))
    else {
        return agent.type_error("the species constructor did not return an ArrayBuffer");
    };
    let Some(created_length) = agent.buffer_len(new) else {
        return detached_error(agent);
    };
    if new == buffer {
        return agent.type_error("the species constructor returned the same ArrayBuffer");
    }
    agent.require_mutable_buffer(new)?;
    if created_length < new_length {
        return agent.type_error("the species constructor returned a smaller ArrayBuffer");
    }
    // The constructor ran script, which may have detached or shrunk us.
    let Some(current) = agent.buffer_len(buffer) else {
        return detached_error(agent);
    };
    let end = last.min(current);
    if first < end {
        let source = agent.array_buffer_bytes(buffer).expect("attached")[first..end].to_vec();
        let target = agent
            .buffer_data_mut(new)
            .and_then(|d| d.bytes.as_mut())
            .expect("attached");
        target[..source.len()].copy_from_slice(&source);
    }
    Ok(created)
}

/// `ArrayBuffer.prototype.resize(newLength)` (§25.1.6.6).
fn buffer_resize(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let buffer = this_buffer(agent, this, "resize")?;
    let Some(max) = agent.buffer_data(buffer).and_then(|d| d.max) else {
        return agent.type_error("ArrayBuffer.prototype.resize: the buffer is not resizable");
    };
    let new_length = agent.to_index(arg(args, 0))?;
    let Some(old_length) = agent.buffer_len(buffer) else {
        return detached_error(agent);
    };
    let new_length = match usize::try_from(new_length) {
        Ok(n) if n <= max => n,
        _ => {
            return Err(agent.throw(
                ErrorKind::RangeError,
                "ArrayBuffer.prototype.resize: the length exceeds maxByteLength",
            ));
        }
    };
    if new_length > old_length && agent.heap.charge(new_length - old_length).is_err() {
        return Err(agent.throw(ErrorKind::RangeError, "Array buffer allocation failed"));
    }
    let bytes = agent
        .buffer_data_mut(buffer)
        .and_then(|d| d.bytes.as_mut())
        .expect("attached");
    bytes.resize(new_length, 0);
    bytes.shrink_to_fit();
    Ok(Value::UNDEFINED)
}

/// `ArrayBufferCopyAndDetach` (§25.1.3.14).
fn copy_and_detach(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
    preserve: bool,
    method: &str,
) -> JsResult<Value> {
    let buffer = this_buffer(agent, this, method)?;
    agent.require_mutable_buffer(buffer)?;
    let requested = arg(args, 0);
    let new_length = if requested == Value::UNDEFINED {
        agent.buffer_len(buffer)
    } else {
        Some(to_byte_index(agent, requested)?)
    };
    let Some(data) = agent.buffer_data(buffer).filter(|d| d.bytes.is_some()) else {
        return detached_error(agent);
    };
    let max = data.max.filter(|_| preserve);
    let new_length = new_length.expect("attached buffers have a length");
    let prototype = agent.intrinsics.array_buffer_prototype;
    let new = agent.allocate_array_buffer(prototype, new_length, max)?;
    let source = agent.array_buffer_bytes(buffer).expect("attached");
    let count = source.len().min(new_length);
    let copy = source[..count].to_vec();
    agent
        .buffer_data_mut(new)
        .and_then(|d| d.bytes.as_mut())
        .expect("attached")[..count]
        .copy_from_slice(&copy);
    agent.detach_array_buffer(buffer);
    Ok(Value::object(new))
}

fn buffer_transfer(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    copy_and_detach(agent, this, args, true, "transfer")
}

fn buffer_transfer_fixed(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    copy_and_detach(agent, this, args, false, "transferToFixedLength")
}

// ---- SharedArrayBuffer (§25.2) ------------------------------------------------

/// The receiver as a SharedArrayBuffer.
fn this_shared(agent: &mut Agent, this: Value, method: &str) -> JsResult<ObjectId> {
    match this
        .as_object()
        .filter(|o| agent.is_shared_array_buffer(*o))
    {
        Some(o) => Ok(o),
        None => agent.type_error(&format!(
            "SharedArrayBuffer.prototype.{method} called on an incompatible receiver"
        )),
    }
}

fn shared_byte_length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let buffer = this_shared(agent, this, "byteLength")?;
    Ok(Value::number(agent.buffer_len(buffer).unwrap_or(0) as f64))
}

fn shared_growable(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let buffer = this_shared(agent, this, "growable")?;
    let data = agent.buffer_data(buffer).expect("checked above");
    Ok(Value::boolean(data.max.is_some()))
}

fn shared_max_byte_length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let buffer = this_shared(agent, this, "maxByteLength")?;
    let data = agent.buffer_data(buffer).expect("checked above");
    let length = data
        .max
        .unwrap_or_else(|| data.bytes.as_ref().map_or(0, Vec::len));
    Ok(Value::number(length as f64))
}

/// `SharedArrayBuffer.prototype.grow(newLength)` (§25.2.5.3): a shared
/// buffer never shrinks.
fn shared_grow(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let buffer = this_shared(agent, this, "grow")?;
    let Some(max) = agent.buffer_data(buffer).and_then(|d| d.max) else {
        return agent.type_error("SharedArrayBuffer.prototype.grow: the buffer is not growable");
    };
    let new_length = agent.to_index(arg(args, 0))?;
    let old_length = agent
        .buffer_len(buffer)
        .expect("shared buffers stay attached");
    let new_length = match usize::try_from(new_length) {
        Ok(n) if n <= max && n >= old_length => n,
        _ => {
            return Err(agent.throw(
                ErrorKind::RangeError,
                "SharedArrayBuffer.prototype.grow: the length is below the current length or above maxByteLength",
            ));
        }
    };
    if agent.heap.charge(new_length - old_length).is_err() {
        return Err(agent.throw(ErrorKind::RangeError, "Array buffer allocation failed"));
    }
    agent
        .buffer_data_mut(buffer)
        .and_then(|d| d.bytes.as_mut())
        .expect("shared buffers stay attached")
        .resize(new_length, 0);
    Ok(Value::UNDEFINED)
}

/// `SharedArrayBuffer.prototype.slice(start, end)` (§25.2.5.6).
fn shared_slice(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let buffer = this_shared(agent, this, "slice")?;
    let length = agent
        .buffer_len(buffer)
        .expect("shared buffers stay attached");
    let first = clamp_relative(agent, arg(args, 0), length, 0)?;
    let last = clamp_relative(agent, arg(args, 1), length, length)?;
    let new_length = last.saturating_sub(first);
    let default = agent.intrinsics.shared_array_buffer;
    let constructor = species_constructor(agent, buffer, default)?;
    let created = agent.construct(
        Value::object(constructor),
        &[Value::number(new_length as f64)],
    )?;
    let Some(new) = created
        .as_object()
        .filter(|o| agent.is_shared_array_buffer(*o))
    else {
        return agent.type_error("the species constructor did not return a SharedArrayBuffer");
    };
    if new == buffer {
        return agent.type_error("the species constructor returned the same SharedArrayBuffer");
    }
    let created_length = agent.buffer_len(new).expect("shared buffers stay attached");
    if created_length < new_length {
        return agent.type_error("the species constructor returned a smaller SharedArrayBuffer");
    }
    if first < last {
        let source = agent.array_buffer_bytes(buffer).expect("attached")[first..last].to_vec();
        let target = agent
            .buffer_data_mut(new)
            .and_then(|d| d.bytes.as_mut())
            .expect("attached");
        target[..source.len()].copy_from_slice(&source);
    }
    Ok(created)
}

// ---- DataView (§25.3) ---------------------------------------------------------

/// `DataView(buffer, byteOffset, byteLength)` (§25.3.2.1).
fn data_view_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if agent.new_target().is_none() {
        return agent.type_error("Constructor DataView requires 'new'");
    }
    let Some(buffer) = arg(args, 0)
        .as_object()
        .filter(|o| agent.buffer_data(*o).is_some())
    else {
        return agent.type_error("DataView: the first argument must be an ArrayBuffer");
    };
    let offset = agent.to_index(arg(args, 1))?;
    let Some(buffer_length) = agent.buffer_len(buffer) else {
        return detached_error(agent);
    };
    let offset = match usize::try_from(offset) {
        Ok(offset) if offset <= buffer_length => offset,
        _ => {
            return Err(agent.throw(
                ErrorKind::RangeError,
                "DataView: the offset is outside the buffer",
            ));
        }
    };
    let resizable = agent.buffer_data(buffer).is_some_and(|d| d.max.is_some());
    let requested = arg(args, 2);
    let length = if requested == Value::UNDEFINED {
        (!resizable).then_some(buffer_length - offset)
    } else {
        let length = agent.to_index(requested)?;
        match usize::try_from(length) {
            Ok(length) if length <= buffer_length - offset => Some(length),
            _ => {
                return Err(agent.throw(
                    ErrorKind::RangeError,
                    "DataView: the length is outside the buffer",
                ));
            }
        }
    };
    let default = agent.intrinsics.data_view_prototype;
    let prototype = prototype_from_new_target(agent, default)?;
    // Getting the prototype ran script: check the buffer again.
    let Some(buffer_length) = agent.buffer_len(buffer) else {
        return detached_error(agent);
    };
    if offset > buffer_length || length.is_some_and(|l| offset + l > buffer_length) {
        return Err(agent.throw(
            ErrorKind::RangeError,
            "DataView: the view is outside the buffer",
        ));
    }
    agent
        .heap
        .charge(std::mem::size_of::<crate::runtime::Extra>())?;
    let mut object = Object::new(Class::DataView, Some(prototype));
    object.extra_mut().internal = Some(Internal::DataView {
        buffer,
        offset,
        length,
    });
    Ok(Value::object(agent.heap.alloc_object(object)?))
}

/// The receiver's `[[ViewedArrayBuffer]]`, `[[ByteOffset]]` and
/// `[[ByteLength]]` (`None` when it tracks the buffer).
fn this_view(agent: &mut Agent, this: Value) -> JsResult<(ObjectId, usize, Option<usize>)> {
    let view = this
        .as_object()
        .and_then(|o| match agent.heap.object(o).internal() {
            Some(Internal::DataView {
                buffer,
                offset,
                length,
            }) => Some((*buffer, *offset, *length)),
            _ => None,
        });
    match view {
        Some(view) => Ok(view),
        None => agent.type_error("DataView method called on an incompatible receiver"),
    }
}

/// `GetViewByteLength` after `IsViewOutOfBounds`: a TypeError when the
/// buffer is detached or no longer covers the view.
fn view_byte_length(
    agent: &mut Agent,
    buffer: ObjectId,
    offset: usize,
    length: Option<usize>,
) -> JsResult<usize> {
    let Some(buffer_length) = agent.buffer_len(buffer) else {
        return detached_error(agent);
    };
    let in_bounds = match length {
        None => offset <= buffer_length,
        Some(length) => offset + length <= buffer_length,
    };
    if !in_bounds {
        return agent.type_error("the DataView is out of the bounds of its buffer");
    }
    Ok(length.unwrap_or(buffer_length - offset))
}

fn data_view_buffer(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (buffer, _, _) = this_view(agent, this)?;
    Ok(Value::object(buffer))
}

/// The view's byte length, or `None` when `IsViewOutOfBounds` (the
/// buffer is detached or no longer covers the view).
fn view_length_in_bounds(
    agent: &Agent,
    buffer: ObjectId,
    offset: usize,
    length: Option<usize>,
) -> Option<usize> {
    let buffer_length = agent.buffer_len(buffer)?;
    match length {
        None => buffer_length.checked_sub(offset),
        Some(length) => (offset + length <= buffer_length).then_some(length),
    }
}

/// `get DataView.prototype.byteLength` (§25.3.4.2): +0 when the view is
/// out of bounds, as the current specification says.
fn data_view_byte_length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (buffer, offset, length) = this_view(agent, this)?;
    let length = view_length_in_bounds(agent, buffer, offset, length).unwrap_or(0);
    Ok(Value::number(length as f64))
}

/// `get DataView.prototype.byteOffset` (§25.3.4.3): +0 when the view is
/// out of bounds.
fn data_view_byte_offset(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (buffer, offset, length) = this_view(agent, this)?;
    let offset = match view_length_in_bounds(agent, buffer, offset, length) {
        Some(_) => offset,
        None => 0,
    };
    Ok(Value::number(offset as f64))
}

/// `GetViewValue` (§25.3.1.5).
fn get_view_value(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
    kind: ElementKind,
) -> JsResult<Value> {
    let (buffer, offset, length) = this_view(agent, this)?;
    let index = agent.to_index(arg(args, 0))?;
    let little = agent.to_boolean(arg(args, 1));
    let view_length = view_byte_length(agent, buffer, offset, length)?;
    let size = kind.size();
    let at = match usize::try_from(index) {
        Ok(index) if index <= view_length && size <= view_length - index => offset + index,
        _ => {
            return Err(agent.throw(
                ErrorKind::RangeError,
                "DataView: the offset is out of range",
            ));
        }
    };
    let mut bits = agent.buffer_read(buffer, at, size);
    if !little {
        bits = bits.swap_bytes() >> (64 - 8 * size);
    }
    agent.element_value(kind, bits)
}

/// `SetViewValue` (§25.3.1.6).
fn set_view_value(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
    kind: ElementKind,
) -> JsResult<Value> {
    let (buffer, offset, length) = this_view(agent, this)?;
    let index = agent.to_index(arg(args, 0))?;
    let mut bits = agent.element_bits(kind, arg(args, 1))?;
    let little = agent.to_boolean(arg(args, 2));
    let view_length = view_byte_length(agent, buffer, offset, length)?;
    let size = kind.size();
    let at = match usize::try_from(index) {
        Ok(index) if index <= view_length && size <= view_length - index => offset + index,
        _ => {
            return Err(agent.throw(
                ErrorKind::RangeError,
                "DataView: the offset is out of range",
            ));
        }
    };
    if !little {
        bits = (bits << (64 - 8 * size)).swap_bytes();
    }
    agent.buffer_write(buffer, at, size, bits)?;
    Ok(Value::UNDEFINED)
}

macro_rules! accessors {
    ($($get:ident, $set:ident => $kind:ident;)*) => {
        $(
            fn $get(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
                get_view_value(agent, this, args, ElementKind::$kind)
            }

            fn $set(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
                set_view_value(agent, this, args, ElementKind::$kind)
            }
        )*
    };
}

accessors! {
    get_int8, set_int8 => Int8;
    get_uint8, set_uint8 => Uint8;
    get_int16, set_int16 => Int16;
    get_uint16, set_uint16 => Uint16;
    get_int32, set_int32 => Int32;
    get_uint32, set_uint32 => Uint32;
    get_float16, set_float16 => Float16;
    get_float32, set_float32 => Float32;
    get_float64, set_float64 => Float64;
    get_big_int64, set_big_int64 => BigInt64;
    get_big_uint64, set_big_uint64 => BigUint64;
}

const DATA_VIEW_METHODS: &[super::Method] = &[
    method("getInt8", get_int8, 1),
    method("setInt8", set_int8, 2),
    method("getUint8", get_uint8, 1),
    method("setUint8", set_uint8, 2),
    method("getInt16", get_int16, 1),
    method("setInt16", set_int16, 2),
    method("getUint16", get_uint16, 1),
    method("setUint16", set_uint16, 2),
    method("getInt32", get_int32, 1),
    method("setInt32", set_int32, 2),
    method("getUint32", get_uint32, 1),
    method("setUint32", set_uint32, 2),
    method("getFloat16", get_float16, 1),
    method("setFloat16", set_float16, 2),
    method("getFloat32", get_float32, 1),
    method("setFloat32", set_float32, 2),
    method("getFloat64", get_float64, 1),
    method("setFloat64", set_float64, 2),
    method("getBigInt64", get_big_int64, 1),
    method("setBigInt64", set_big_int64, 2),
    method("getBigUint64", get_big_uint64, 1),
    method("setBigUint64", set_big_uint64, 2),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary16_round_trips_and_rounds_to_even() {
        for bits in 0..=u16::MAX {
            let value = f16_bits_to_f64(bits);
            if value.is_nan() {
                assert_eq!(f64_to_f16_bits(value), 0x7E00);
            } else {
                assert_eq!(f64_to_f16_bits(value), bits, "{bits:#06x}");
            }
        }
        // Halfway cases go to the even neighbor, in both ranges.
        assert_eq!(f64_to_f16_bits(1.0 + 1.0 / 2048.0), 0x3C00);
        assert_eq!(f64_to_f16_bits(1.0 + 3.0 / 2048.0), 0x3C02);
        assert_eq!(f64_to_f16_bits(f64::powi(2.0, -25)), 0);
        assert_eq!(f64_to_f16_bits(1.5 * f64::powi(2.0, -24)), 2);
        assert_eq!(f64_to_f16_bits(65519.99), 0x7BFF);
        assert_eq!(f64_to_f16_bits(65520.0), 0x7C00);
        assert_eq!(f64_to_f16_bits(-1e30), 0xFC00);
        assert_eq!(f64_to_f16_bits(-0.0), 0x8000);
        assert_eq!(to_uint8_clamp(0.5), 0);
        assert_eq!(to_uint8_clamp(1.5), 2);
        assert_eq!(to_uint8_clamp(300.0), 255);
        assert_eq!(to_uint8_clamp(-3.0), 0);
    }
}
