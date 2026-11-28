//! `DOMTokenList` (DOM §7.1): a live view of an element attribute's
//! ordered set of tokens, for `Element.classList` and
//! `HTMLIFrameElement.sandbox`.
//!
//! A list's source is `[element wrapper, attribute name]`; every read
//! parses the attribute again, so the list follows the attribute. Each
//! element keeps one list per attribute (`[SameObject]`), held in a weak
//! map keyed by the element's wrapper.

use rustic_dom::{DomString, NodeId, NodeKind, QualifiedName};
use rustic_js::runtime::{
    Abrupt, Agent, JsResult, LiveIndexAccess, ObjectId, PropertyDescriptor, PropertyKey, Value,
};

use crate::dom_exception;
use crate::webidl::{DomStringConversion, IntegerConversion, to_dom_string, to_unsigned_long};

use super::traversal::{node_of, read_doc};
use super::{
    illegal_constructor, install_interface, prototype, required_arg, with_mut_tree, with_page,
};

const TOKEN_LIST: &str = "DOMTokenList";

/// The attributes with a token list, by index into the page's weak maps.
pub(super) const LISTED: [&str; 2] = ["class", "sandbox"];

/// The tokens `iframe` `sandbox` supports (HTML §4.8.5).
const SANDBOX_TOKENS: [&str; 13] = [
    "allow-downloads",
    "allow-forms",
    "allow-modals",
    "allow-orientation-lock",
    "allow-pointer-lock",
    "allow-popups",
    "allow-popups-to-escape-sandbox",
    "allow-presentation",
    "allow-same-origin",
    "allow-scripts",
    "allow-top-navigation",
    "allow-top-navigation-by-user-activation",
    "allow-top-navigation-to-custom-protocols",
];

/// Page-lifetime state: one weak map per entry of [`LISTED`].
#[derive(Debug)]
pub(super) struct TokenLists {
    lists: [ObjectId; LISTED.len()],
}

impl TokenLists {
    pub(super) fn new(agent: &mut Agent) -> JsResult<Self> {
        let mut map = || -> JsResult<ObjectId> {
            let map = agent.new_weak_map()?;
            agent.root(Value::object(map))?;
            Ok(map)
        };
        Ok(Self {
            lists: [map()?, map()?],
        })
    }
}

static ACCESS: LiveIndexAccess = LiveIndexAccess {
    length: live_length,
    item: live_item,
    named: None,
};

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        TOKEN_LIST,
        None,
        illegal_constructor,
        0,
        &[("length", "get length", length)],
        |agent, proto| {
            for (name, call, arity) in [
                ("item", item as rustic_js::runtime::NativeFn, 1),
                ("contains", contains, 1),
                ("add", add, 0),
                ("remove", remove, 0),
                ("toggle", toggle, 1),
                ("replace", replace, 2),
                ("supports", supports, 1),
                ("toString", value, 0),
            ] {
                let key = agent.heap.key(name)?;
                agent.define_method(proto, key, call, name, arity)?;
            }
            let get = Value::object(agent.new_native_function(value, "get value", 0)?);
            let set = Value::object(agent.new_native_function(set_value, "set value", 1)?);
            let key = agent.heap.key("value")?;
            agent.define_property_or_throw(
                proto,
                key,
                PropertyDescriptor::accessor(get, set, true, true),
            )?;
            for name in ["entries", "keys", "values", "forEach"] {
                let key = agent.heap.key(name)?;
                let method = agent.get(agent.intrinsics.array_prototype, key)?;
                agent.define_property_or_throw(
                    proto,
                    key,
                    PropertyDescriptor::data_with(method, true, false, true),
                )?;
            }
            agent.define_property_or_throw(
                proto,
                PropertyKey::Symbol(agent.symbols.iterator),
                PropertyDescriptor::data_with(
                    Value::object(agent.intrinsics.array_values),
                    true,
                    false,
                    true,
                ),
            )
        },
    )?;
    // `Element.classList`, `[SameObject, PutForwards=value]`.
    let element = prototype(agent, "Element")?;
    define_list_accessor(agent, element, "classList", class_list, set_class_list)
}

/// Define a `[SameObject, PutForwards=value]` token-list attribute.
pub(super) fn define_list_accessor(
    agent: &mut Agent,
    proto: ObjectId,
    name: &str,
    get: rustic_js::runtime::NativeFn,
    set: rustic_js::runtime::NativeFn,
) -> JsResult<()> {
    let get = Value::object(agent.new_native_function(get, &format!("get {name}"), 0)?);
    let set = Value::object(agent.new_native_function(set, &format!("set {name}"), 1)?);
    let key = agent.heap.key(name)?;
    agent.define_property_or_throw(
        proto,
        key,
        PropertyDescriptor::accessor(get, set, true, true),
    )
}

fn class_list(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    list_of(agent, this, 0)
}

fn set_class_list(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let list = list_of(agent, this, 0)?;
    set_value(agent, list, args)
}

/// The token list of `element`'s [`LISTED`]`[which]` attribute.
pub(super) fn list_of(agent: &mut Agent, element: Value, which: usize) -> JsResult<Value> {
    let id = node_of(agent, element)?;
    if !read_doc(agent, id, |d| {
        matches!(d.node(id).map(|n| n.kind()), Ok(NodeKind::Element(_)))
    })? {
        return agent.type_error("expected an Element receiver");
    }
    let map = with_page(agent, |_, page| Ok(page.token_lists.lists[which]))?;
    let known = agent.weak_map_get(map, element);
    if known.as_object().is_some() {
        return Ok(known);
    }
    let name = Value::string(agent.heap.intern_str(LISTED[which])?);
    let source = agent.create_array_from_list(&[element, name])?;
    let proto = prototype(agent, TOKEN_LIST)?;
    let list =
        Value::object(agent.new_indexed_view(TOKEN_LIST, proto, Value::object(source), &ACCESS)?);
    agent.weak_map_set(map, element, list)?;
    Ok(list)
}

/// The element and attribute behind a list's source array.
fn source_parts(agent: &mut Agent, source: Value) -> JsResult<(Value, String)> {
    let array = source.as_object().expect("token list source is an array");
    let element = agent.get(array, PropertyKey::Index(0))?;
    let name = agent.get(array, PropertyKey::Index(1))?;
    let name = name.as_string().expect("token list attribute name");
    let name = agent
        .heap
        .string(name)
        .units()
        .map(|u| char::from_u32(u32::from(u)).unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect::<String>();
    Ok((element, name))
}

fn receiver(agent: &mut Agent, this: Value) -> JsResult<(Value, String)> {
    let source = this
        .as_object()
        .and_then(|object| agent.indexed_host_source(object, TOKEN_LIST));
    match source {
        Some(source) => source_parts(agent, source),
        None => agent.type_error("expected a DOMTokenList receiver"),
    }
}

/// The attribute's value, or `None` when it is absent.
fn attribute(agent: &mut Agent, id: NodeId, name: &str) -> JsResult<Option<String>> {
    read_doc(agent, id, |d| match d.node(id).map(|n| n.kind()) {
        Ok(NodeKind::Element(e)) => e
            .attributes()
            .iter()
            .find(|a| a.name().namespace().is_none() && a.name().local() == name)
            .map(|a| String::from_utf16_lossy(a.value().as_units())),
        _ => None,
    })
}

/// The ordered set parser (DOM §2.3): ASCII-whitespace-separated tokens,
/// first occurrences kept.
fn ordered_set(value: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for token in value
        .split(['\t', '\n', '\x0c', '\r', ' '])
        .filter(|token| !token.is_empty())
    {
        if !tokens.iter().any(|known| known == token) {
            tokens.push(token.to_owned());
        }
    }
    tokens
}

fn tokens(
    agent: &mut Agent,
    element: Value,
    name: &str,
) -> JsResult<(NodeId, Option<String>, Vec<String>)> {
    let id = node_of(agent, element)?;
    let value = attribute(agent, id, name)?;
    let set = ordered_set(value.as_deref().unwrap_or(""));
    Ok((id, value, set))
}

fn live_length(agent: &mut Agent, source: Value) -> JsResult<u32> {
    let (element, name) = source_parts(agent, source)?;
    let (_, _, set) = tokens(agent, element, &name)?;
    Ok(u32::try_from(set.len()).unwrap_or(u32::MAX))
}

fn live_item(agent: &mut Agent, source: Value, index: u32) -> JsResult<Option<Value>> {
    let (element, name) = source_parts(agent, source)?;
    let (_, _, set) = tokens(agent, element, &name)?;
    match set.get(index as usize) {
        Some(token) => Ok(Some(Value::string(agent.heap.intern_str(token)?))),
        None => Ok(None),
    }
}

fn length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (element, name) = receiver(agent, this)?;
    let (_, _, set) = tokens(agent, element, &name)?;
    Ok(Value::number(set.len() as f64))
}

fn item(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (element, name) = receiver(agent, this)?;
    let index = required_arg(agent, args, 0)?;
    let index = to_unsigned_long(agent, index, IntegerConversion::Wrap)?;
    let (_, _, set) = tokens(agent, element, &name)?;
    match set.get(index as usize) {
        Some(token) => Ok(Value::string(agent.heap.intern_str(token)?)),
        None => Ok(Value::NULL),
    }
}

fn string(agent: &mut Agent, value: Value) -> JsResult<String> {
    let value = to_dom_string(agent, value, DomStringConversion::Default)?;
    Ok(String::from_utf16_lossy(value.as_units()))
}

fn throw(agent: &mut Agent, name: &str, message: &str) -> Abrupt {
    match dom_exception::new(agent, name, message) {
        Ok(error) => Abrupt::Throw(error),
        Err(error) => error,
    }
}

/// A token argument: empty is a `SyntaxError`, ASCII whitespace an
/// `InvalidCharacterError` (DOM §7.1 add, remove, toggle and replace).
fn validate(agent: &mut Agent, token: &str) -> JsResult<()> {
    if token.is_empty() {
        return Err(throw(agent, "SyntaxError", "the token is empty"));
    }
    if token.contains(['\t', '\n', '\x0c', '\r', ' ']) {
        return Err(throw(
            agent,
            "InvalidCharacterError",
            "the token contains whitespace",
        ));
    }
    Ok(())
}

/// The update steps: nothing when the attribute is absent and the set
/// empty, else the serialized set.
fn update(
    agent: &mut Agent,
    element: Value,
    name: &str,
    had: bool,
    set: &[String],
) -> JsResult<()> {
    if !had && set.is_empty() {
        return Ok(());
    }
    set_attribute(agent, element, name, &set.join(" "))
}

fn set_attribute(agent: &mut Agent, element: Value, name: &str, value: &str) -> JsResult<()> {
    with_mut_tree(agent, element, |agent, document, _, id| {
        let qualified = QualifiedName::from_parsed(None, None, name)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        document
            .set_attribute(id, qualified, DomString::from(value))
            .map(|_| ())
            .map_err(|error| dom_exception::from_dom_error(agent, error))
    })
}

fn contains(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (element, name) = receiver(agent, this)?;
    let token = required_arg(agent, args, 0)?;
    let token = string(agent, token)?;
    let (_, _, set) = tokens(agent, element, &name)?;
    Ok(Value::boolean(set.contains(&token)))
}

fn arguments(agent: &mut Agent, args: &[Value]) -> JsResult<Vec<String>> {
    let mut all = Vec::with_capacity(args.len());
    for &arg in args {
        all.push(string(agent, arg)?);
    }
    for token in &all {
        validate(agent, token)?;
    }
    Ok(all)
}

fn add(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (element, name) = receiver(agent, this)?;
    let added = arguments(agent, args)?;
    let (_, value, mut set) = tokens(agent, element, &name)?;
    for token in added {
        if !set.contains(&token) {
            set.push(token);
        }
    }
    update(agent, element, &name, value.is_some(), &set)?;
    Ok(Value::UNDEFINED)
}

fn remove(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (element, name) = receiver(agent, this)?;
    let removed = arguments(agent, args)?;
    let (_, value, mut set) = tokens(agent, element, &name)?;
    set.retain(|token| !removed.contains(token));
    update(agent, element, &name, value.is_some(), &set)?;
    Ok(Value::UNDEFINED)
}

fn toggle(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (element, name) = receiver(agent, this)?;
    let token = required_arg(agent, args, 0)?;
    let token = string(agent, token)?;
    validate(agent, &token)?;
    let force = match args.get(1).copied() {
        None => None,
        Some(v) if v == Value::UNDEFINED => None,
        Some(v) => Some(agent.to_boolean(v)),
    };
    let (_, value, mut set) = tokens(agent, element, &name)?;
    if set.contains(&token) {
        if force == Some(true) {
            return Ok(Value::TRUE);
        }
        set.retain(|known| *known != token);
        update(agent, element, &name, value.is_some(), &set)?;
        return Ok(Value::FALSE);
    }
    if force == Some(false) {
        return Ok(Value::FALSE);
    }
    set.push(token);
    update(agent, element, &name, value.is_some(), &set)?;
    Ok(Value::TRUE)
}

fn replace(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (element, name) = receiver(agent, this)?;
    let old = required_arg(agent, args, 0)?;
    let old = string(agent, old)?;
    let new = required_arg(agent, args, 1)?;
    let new = string(agent, new)?;
    validate(agent, &old)?;
    validate(agent, &new)?;
    let (_, value, mut set) = tokens(agent, element, &name)?;
    let Some(at) = set.iter().position(|token| *token == old) else {
        return Ok(Value::FALSE);
    };
    // The ordered-set replace: the first of `old` and `new` becomes `new`,
    // every other occurrence of either goes.
    let first = set
        .iter()
        .position(|token| *token == new)
        .map_or(at, |n| n.min(at));
    set[first] = new.clone();
    let mut index = 0;
    set.retain(|token| {
        let keep = index == first || (*token != old && *token != new);
        index += 1;
        keep
    });
    update(agent, element, &name, value.is_some(), &set)?;
    Ok(Value::TRUE)
}

fn supports(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, name) = receiver(agent, this)?;
    let token = required_arg(agent, args, 0)?;
    let token = string(agent, token)?.to_ascii_lowercase();
    match name.as_str() {
        "sandbox" => Ok(Value::boolean(SANDBOX_TOKENS.contains(&token.as_str()))),
        _ => agent.type_error(&format!("the {name} attribute defines no supported tokens")),
    }
}

fn value(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (element, name) = receiver(agent, this)?;
    let id = node_of(agent, element)?;
    let value = attribute(agent, id, &name)?.unwrap_or_default();
    Ok(Value::string(agent.heap.intern_str(&value)?))
}

fn set_value(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (element, name) = receiver(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let value = string(agent, value)?;
    set_attribute(agent, element, &name, &value)?;
    Ok(Value::UNDEFINED)
}

#[cfg(test)]
mod tests {
    use super::ordered_set;

    #[test]
    fn the_ordered_set_parser_drops_whitespace_and_repeats() {
        assert_eq!(ordered_set("  a\tb a\nc  "), ["a", "b", "c"]);
        assert!(ordered_set(" \x0c ").is_empty());
    }
}
