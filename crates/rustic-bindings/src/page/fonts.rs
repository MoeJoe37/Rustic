//! CSS Font Loading 3: the `FontFace` interface and `document.fonts`
//! (`FontFaceSet`).
//!
//! A `FontFace` built from binary data is `loaded` when the bytes start with
//! an OpenType, TrueType, WOFF, WOFF2 or collection signature, else
//! `error` (its `loaded` promise rejects with a `SyntaxError`). Faces with
//! a CSS `src` string cannot be fetched from script yet: `load()` rejects
//! with a `NetworkError`. The set's faces do not reach layout, which uses the
//! document's `@font-face` rules; the set is always `loaded` and `ready`
//! is already fulfilled, because those rules load before the first layout
//! (TD-212).

use rustic_js::runtime::{Abrupt, Agent, JsResult, ObjectId, PropertyKey, Value};

use super::events::{PrivateArray, define_method, with_root};
use super::{creation, illegal_constructor, install_interface, prototype, with_page};

const FONT_FACE: &str = "FontFace";
const FONT_FACE_SET: &str = "FontFaceSet";

/// `FontFace` slots: the descriptors in [`DESCRIPTORS`] order, then the
/// status and the `loaded` promise.
const DESCRIPTORS: [(&str, &str); 11] = [
    ("style", "normal"),
    ("weight", "normal"),
    ("stretch", "normal"),
    ("unicodeRange", "U+0-10FFFF"),
    ("featureSettings", "normal"),
    ("variationSettings", "normal"),
    ("display", "auto"),
    ("ascentOverride", "normal"),
    ("descentOverride", "normal"),
    ("lineGapOverride", "normal"),
    ("family", ""),
];
const S_FAMILY: u32 = 10;
const S_STATUS: u32 = 11;
const S_LOADED: u32 = 12;
const S_BINARY: u32 = 13;

/// Page-lifetime font loading state.
#[derive(Debug)]
pub(super) struct Fonts {
    /// `FontFace` object → slots.
    faces: ObjectId,
    /// The document's `FontFaceSet` and its private face list.
    set: ObjectId,
    members: ObjectId,
}

impl Fonts {
    pub(super) fn new(agent: &mut Agent) -> JsResult<Self> {
        let faces = agent.new_weak_map()?;
        agent.root(Value::object(faces))?;
        let proto = prototype(agent, FONT_FACE_SET)?;
        let set = agent.new_object_with(Some(proto))?;
        agent.root(Value::object(set))?;
        let members = PrivateArray::new(agent, &[])?;
        agent.root(members.value())?;
        let members = members.value().as_object().expect("private array");
        Ok(Self {
            faces,
            set,
            members,
        })
    }
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let mut attributes: Vec<(&str, &str, rustic_js::runtime::NativeFn)> = vec![
        ("family", "get family", descriptor::<S_FAMILY>),
        ("style", "get style", descriptor::<0>),
        ("weight", "get weight", descriptor::<1>),
        ("stretch", "get stretch", descriptor::<2>),
        ("unicodeRange", "get unicodeRange", descriptor::<3>),
        ("featureSettings", "get featureSettings", descriptor::<4>),
        (
            "variationSettings",
            "get variationSettings",
            descriptor::<5>,
        ),
        ("display", "get display", descriptor::<6>),
        ("ascentOverride", "get ascentOverride", descriptor::<7>),
        ("descentOverride", "get descentOverride", descriptor::<8>),
        ("lineGapOverride", "get lineGapOverride", descriptor::<9>),
    ];
    attributes.push(("status", "get status", descriptor::<S_STATUS>));
    attributes.push(("loaded", "get loaded", descriptor::<S_LOADED>));
    install_interface(
        agent,
        FONT_FACE,
        None,
        construct,
        2,
        &attributes,
        |agent, proto| define_method(agent, proto, "load", load, 0),
    )?;
    install_interface(
        agent,
        FONT_FACE_SET,
        None,
        illegal_constructor,
        0,
        &[
            ("size", "get size", size),
            ("status", "get status", set_status),
            ("ready", "get ready", ready),
        ],
        |agent, proto| {
            define_method(agent, proto, "add", add, 1)?;
            define_method(agent, proto, "delete", delete, 1)?;
            define_method(agent, proto, "has", has, 1)?;
            define_method(agent, proto, "clear", clear, 0)?;
            define_method(agent, proto, "forEach", for_each, 1)?;
            define_method(agent, proto, "check", check, 1)?;
            define_method(agent, proto, "load", set_load, 1)
        },
    )?;
    let document = prototype(agent, "Document")?;
    let getter = Value::object(agent.new_native_function(document_fonts, "get fonts", 0)?);
    let key = agent.heap.key("fonts")?;
    agent.define_property_or_throw(
        document,
        key,
        rustic_js::runtime::PropertyDescriptor::accessor(getter, Value::UNDEFINED, true, true),
    )
}

fn text(agent: &mut Agent, value: &str) -> JsResult<Value> {
    Ok(Value::string(agent.heap.intern_str(value)?))
}

fn string(agent: &mut Agent, value: Value) -> JsResult<String> {
    let id = agent.to_string(value)?;
    Ok(char::decode_utf16(agent.heap.string(id).units())
        .map(|c| c.unwrap_or('\u{FFFD}'))
        .collect())
}

fn error(agent: &mut Agent, name: &str, message: &str) -> JsResult<Value> {
    match creation::exception(agent, name, message) {
        Abrupt::Throw(value) => Ok(value),
        other => Err(other),
    }
}

/// Whether `bytes` start like a font file Rustic decodes.
fn font_signature(bytes: &[u8]) -> bool {
    matches!(
        bytes.get(..4),
        Some([0, 1, 0, 0] | b"OTTO" | b"true" | b"wOFF" | b"wOF2" | b"ttcf")
    )
}

/// `new FontFace(family, source, descriptors)`.
fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("FontFace construction requires new");
    };
    let family = string(agent, args.first().copied().unwrap_or(Value::UNDEFINED))?;
    let source = args.get(1).copied().unwrap_or(Value::UNDEFINED);
    let binary = if agent.is_web_buffer_source(source) {
        let buffer = agent.web_unshared_buffer_source(source)?;
        Some(font_signature(&agent.web_buffer_bytes(buffer)))
    } else {
        string(agent, source)?;
        None
    };
    let options = args.get(2).copied().unwrap_or(Value::UNDEFINED);
    let mut values = Vec::with_capacity(DESCRIPTORS.len() + 3);
    for (name, default) in DESCRIPTORS {
        let given = match options.as_object() {
            Some(options) if name != "family" => {
                let key = agent.heap.key(name)?;
                agent.get(options, key)?
            }
            _ => Value::UNDEFINED,
        };
        let value = if name == "family" {
            family.clone()
        } else if given == Value::UNDEFINED {
            default.to_owned()
        } else {
            string(agent, given)?
        };
        values.push(text(agent, &value)?);
    }
    let proto = agent.host_prototype_from_new_target(FONT_FACE, target)?;
    let face = Value::object(agent.new_object_with(Some(proto))?);
    with_root(agent, face, |agent| {
        let loaded = agent.new_host_promise()?;
        let status = match binary {
            Some(true) => "loaded",
            Some(false) => "error",
            None => "unloaded",
        };
        values.push(text(agent, status)?);
        values.push(Value::object(loaded));
        values.push(Value::boolean(binary.is_some()));
        let slots = PrivateArray::new(agent, &values)?;
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.fonts.faces, face, slots.value())
        })?;
        match binary {
            Some(true) => agent.resolve_host_promise(loaded, face)?,
            Some(false) => {
                let reason = error(agent, "SyntaxError", "the font data is not a font")?;
                agent.reject_host_promise(loaded, reason);
            }
            None => {}
        }
        Ok(face)
    })
}

fn slots_of(agent: &mut Agent, this: Value) -> JsResult<PrivateArray> {
    let slots = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.fonts.faces, this))
    })?;
    match PrivateArray::from_value(slots) {
        Some(slots) => Ok(slots),
        None => agent.type_error("expected a FontFace receiver"),
    }
}

fn descriptor<const SLOT: u32>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    slots_of(agent, this)?.get(agent, SLOT)
}

/// `FontFace.load()`: binary faces have settled already; a `src` string
/// cannot be fetched from script yet.
fn load(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let slots = slots_of(agent, this)?;
    let loaded = slots.get(agent, S_LOADED)?;
    let binary = slots.get(agent, S_BINARY)? == Value::boolean(true);
    let status = slots.get(agent, S_STATUS)?;
    let unloaded = text(agent, "unloaded")?;
    if !binary && agent.same_value(status, unloaded) {
        let failed = text(agent, "error")?;
        slots.set(agent, S_STATUS, failed)?;
        let reason = error(
            agent,
            "NetworkError",
            "fonts cannot be fetched from script yet",
        )?;
        if let Some(promise) = loaded.as_object() {
            agent.reject_host_promise(promise, reason);
        }
    }
    Ok(loaded)
}

fn document_fonts(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    with_page(agent, |_, page| Ok(Value::object(page.fonts.set)))
}

fn members(agent: &mut Agent, this: Value) -> JsResult<PrivateArray> {
    with_page(agent, |agent, page| {
        if this != Value::object(page.fonts.set) {
            return agent.type_error("expected a FontFaceSet receiver");
        }
        Ok(PrivateArray::from_value(Value::object(page.fonts.members)).expect("object"))
    })
}

fn list(agent: &mut Agent, members: PrivateArray) -> JsResult<Vec<Value>> {
    let len = members.len(agent)?;
    (0..len).map(|i| members.get(agent, i)).collect()
}

/// Make the set's rooted private list hold exactly `values`.
fn replace(agent: &mut Agent, values: &[Value]) -> JsResult<()> {
    let list = with_page(agent, |_, page| Ok(page.fonts.members))?;
    let key = PropertyKey::String(agent.names.length);
    agent.set(list, key, Value::number(0.0), true)?;
    let members = PrivateArray::from_value(Value::object(list)).expect("object");
    for (index, &value) in values.iter().enumerate() {
        members.set(agent, index as u32, value)?;
    }
    Ok(())
}

fn size(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let members = members(agent, this)?;
    Ok(Value::number(f64::from(members.len(agent)?)))
}

fn set_status(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    members(agent, this)?;
    text(agent, "loaded")
}

fn ready(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    members(agent, this)?;
    let promise = agent.new_host_promise()?;
    agent.resolve_host_promise(promise, this)?;
    Ok(Value::object(promise))
}

fn face_argument(agent: &mut Agent, args: &[Value]) -> JsResult<Value> {
    let face = args.first().copied().unwrap_or(Value::UNDEFINED);
    slots_of(agent, face)?;
    Ok(face)
}

fn add(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let members = members(agent, this)?;
    let face = face_argument(agent, args)?;
    let mut values = list(agent, members)?;
    if !values.contains(&face) {
        values.push(face);
        replace(agent, &values)?;
    }
    Ok(this)
}

fn delete(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let members = members(agent, this)?;
    let face = args.first().copied().unwrap_or(Value::UNDEFINED);
    let mut values = list(agent, members)?;
    let before = values.len();
    values.retain(|&v| v != face);
    let removed = values.len() != before;
    if removed {
        replace(agent, &values)?;
    }
    Ok(Value::boolean(removed))
}

fn has(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let members = members(agent, this)?;
    let face = args.first().copied().unwrap_or(Value::UNDEFINED);
    Ok(Value::boolean(list(agent, members)?.contains(&face)))
}

fn clear(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    members(agent, this)?;
    replace(agent, &[])?;
    Ok(Value::UNDEFINED)
}

fn for_each(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let members = members(agent, this)?;
    let callback = args.first().copied().unwrap_or(Value::UNDEFINED);
    if !agent.is_callable(callback) {
        return agent.type_error("forEach expects a callback");
    }
    let this_arg = args.get(1).copied().unwrap_or(Value::UNDEFINED);
    for face in list(agent, members)? {
        agent.call(callback, this_arg, &[face, face, this])?;
    }
    Ok(Value::UNDEFINED)
}

/// `check(font, text)`: nothing is left to load, so every font is ready.
fn check(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    members(agent, this)?;
    string(agent, args.first().copied().unwrap_or(Value::UNDEFINED))?;
    Ok(Value::boolean(true))
}

/// `load(font, text)`: fulfilled with the set's faces whose family the
/// font shorthand names.
fn set_load(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let members = members(agent, this)?;
    let font = string(agent, args.first().copied().unwrap_or(Value::UNDEFINED))?;
    let font = font.to_ascii_lowercase();
    let mut matched = Vec::new();
    for face in list(agent, members)? {
        let family = slots_of(agent, face)?.get(agent, S_FAMILY)?;
        let family = string(agent, family)?.to_ascii_lowercase();
        let quoted = |q: char| format!("{q}{family}{q}");
        if !family.is_empty()
            && font.split(',').map(str::trim).any(|part| {
                part.ends_with(&family)
                    || part.ends_with(&quoted('"'))
                    || part.ends_with(&quoted('\''))
            })
        {
            matched.push(face);
        }
    }
    let array = agent.create_array_from_list(&matched)?;
    let promise = agent.new_host_promise()?;
    agent.resolve_host_promise(promise, Value::object(array))?;
    Ok(Value::object(promise))
}
