//! Embedded content: `HTMLImageElement` (HTML §4.8.4) and
//! `HTMLIFrameElement` (§4.8.5) and their reflected content attributes,
//! `loading` (§2.5.7) among them, and the iframe's `sandbox` token list.
//!
//! Loading itself (srcset selection, lazy loading, sandboxed and `srcdoc`
//! frames) happens where documents are rendered; these interfaces expose
//! the attributes that drive it. `width`/`height` reflect as content
//! attributes; rendered sizes, `naturalWidth`, `complete`, `decode()` and
//! `contentWindow` are not implemented (TD-218).

use rustic_dom::{DomString, NodeKind, QualifiedName};
use rustic_js::runtime::{Agent, JsResult, NativeFn, ObjectId, PropertyDescriptor, Value};
use rustic_url::Url;

use crate::dom_exception;
use crate::webidl::{DomStringConversion, to_dom_string};

use super::token_list;
use super::traversal::{node_of, read_doc};
use super::{illegal_constructor, install_interface, with_mut_tree, with_page};

pub(super) const IMAGE: &str = "HTMLImageElement";
pub(super) const IFRAME: &str = "HTMLIFrameElement";

/// How a reflected attribute converts (HTML §2.6.1).
#[derive(Clone, Copy)]
enum Kind {
    Text,
    Url,
    Boolean,
    /// Limited to known values: the keywords and the missing/invalid default.
    Keywords(&'static [&'static str], &'static str),
}

const LOADING: Kind = Kind::Keywords(&["eager", "lazy"], "eager");
const REFERRER: Kind = Kind::Keywords(
    &[
        "",
        "no-referrer",
        "no-referrer-when-downgrade",
        "same-origin",
        "origin",
        "strict-origin",
        "origin-when-cross-origin",
        "strict-origin-when-cross-origin",
        "unsafe-url",
    ],
    "",
);

/// Every reflected attribute: the IDL name, the content attribute, the
/// conversion and the element it belongs to.
const REFLECTED: [(&str, &str, Kind, &str); 18] = [
    ("alt", "alt", Kind::Text, "img"),
    ("src", "src", Kind::Url, "img"),
    ("srcset", "srcset", Kind::Text, "img"),
    ("sizes", "sizes", Kind::Text, "img"),
    ("useMap", "usemap", Kind::Text, "img"),
    ("isMap", "ismap", Kind::Boolean, "img"),
    ("loading", "loading", LOADING, "img"),
    (
        "decoding",
        "decoding",
        Kind::Keywords(&["sync", "async", "auto"], "auto"),
        "img",
    ),
    ("referrerPolicy", "referrerpolicy", REFERRER, "img"),
    ("src", "src", Kind::Url, "iframe"),
    ("srcdoc", "srcdoc", Kind::Text, "iframe"),
    ("name", "name", Kind::Text, "iframe"),
    ("allow", "allow", Kind::Text, "iframe"),
    (
        "allowFullscreen",
        "allowfullscreen",
        Kind::Boolean,
        "iframe",
    ),
    ("width", "width", Kind::Text, "iframe"),
    ("height", "height", Kind::Text, "iframe"),
    ("referrerPolicy", "referrerpolicy", REFERRER, "iframe"),
    ("loading", "loading", LOADING, "iframe"),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        IMAGE,
        Some("HTMLElement"),
        illegal_constructor,
        0,
        &[],
        |agent, proto| define_reflected(agent, proto, "img"),
    )?;
    install_interface(
        agent,
        IFRAME,
        Some("HTMLElement"),
        illegal_constructor,
        0,
        &[],
        |agent, proto| {
            define_reflected(agent, proto, "iframe")?;
            token_list::define_list_accessor(agent, proto, "sandbox", sandbox, set_sandbox)
        },
    )
}

/// The reflectors, by index into [`REFLECTED`].
const GETTERS: [NativeFn; 18] = [
    get::<0>, get::<1>, get::<2>, get::<3>, get::<4>, get::<5>, get::<6>, get::<7>, get::<8>,
    get::<9>, get::<10>, get::<11>, get::<12>, get::<13>, get::<14>, get::<15>, get::<16>,
    get::<17>,
];
const SETTERS: [NativeFn; 18] = [
    set::<0>, set::<1>, set::<2>, set::<3>, set::<4>, set::<5>, set::<6>, set::<7>, set::<8>,
    set::<9>, set::<10>, set::<11>, set::<12>, set::<13>, set::<14>, set::<15>, set::<16>,
    set::<17>,
];

fn define_reflected(agent: &mut Agent, proto: ObjectId, element: &str) -> JsResult<()> {
    for (index, &(name, _, _, owner)) in REFLECTED.iter().enumerate() {
        if owner != element {
            continue;
        }
        let get =
            Value::object(agent.new_native_function(GETTERS[index], &format!("get {name}"), 0)?);
        let set =
            Value::object(agent.new_native_function(SETTERS[index], &format!("set {name}"), 1)?);
        let key = agent.heap.key(name)?;
        agent.define_property_or_throw(
            proto,
            key,
            PropertyDescriptor::accessor(get, set, true, true),
        )?;
    }
    Ok(())
}

/// The value of `name` on the `local` HTML element behind `this`.
fn content(agent: &mut Agent, this: Value, local: &str, name: &str) -> JsResult<Option<String>> {
    let id = node_of(agent, this)?;
    let found = read_doc(agent, id, |d| match d.node(id).map(|n| n.kind()) {
        Ok(NodeKind::Element(e))
            if e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE)
                && e.name().local() == local =>
        {
            Some(
                e.attributes()
                    .iter()
                    .find(|a| a.name().namespace().is_none() && a.name().local() == name)
                    .map(|a| String::from_utf16_lossy(a.value().as_units())),
            )
        }
        _ => None,
    })?;
    match found {
        Some(value) => Ok(value),
        None => agent.type_error(&format!("expected an HTML {local} element receiver")),
    }
}

fn get<const I: usize>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, attribute, kind, local) = REFLECTED[I];
    let value = content(agent, this, local, attribute)?;
    match kind {
        Kind::Boolean => Ok(Value::boolean(value.is_some())),
        Kind::Text => agent.string(value.as_deref().unwrap_or("")),
        Kind::Url => {
            let Some(value) = value else {
                return agent.string("");
            };
            let base = with_page(agent, |_, page| {
                Ok(page
                    .fetch
                    .borrow()
                    .as_ref()
                    .and_then(|host| host.base_url()))
            })?;
            match Url::parse(&value, base.as_ref()) {
                Ok(url) => agent.string(&url.serialize()),
                Err(_) => agent.string(&value),
            }
        }
        Kind::Keywords(keywords, default) => {
            let keyword = value
                .and_then(|value| {
                    keywords
                        .iter()
                        .find(|keyword| keyword.eq_ignore_ascii_case(&value))
                        .copied()
                })
                .unwrap_or(default);
            agent.string(keyword)
        }
    }
}

fn set<const I: usize>(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, attribute, kind, local) = REFLECTED[I];
    // The receiver check, before any conversion side effect.
    content(agent, this, local, attribute)?;
    let arg = args.first().copied().unwrap_or(Value::UNDEFINED);
    let value = match kind {
        Kind::Boolean => agent.to_boolean(arg).then(DomString::default),
        _ => Some(to_dom_string(agent, arg, DomStringConversion::Default)?),
    };
    with_mut_tree(agent, this, |agent, document, _, id| {
        let result = match value {
            Some(value) => {
                let name = QualifiedName::from_parsed(None, None, attribute)
                    .map_err(|error| dom_exception::from_dom_error(agent, error))?;
                document.set_attribute(id, name, value).map(|_| ())
            }
            None => document.remove_attribute(id, None, attribute).map(|_| ()),
        };
        result.map_err(|error| dom_exception::from_dom_error(agent, error))
    })?;
    Ok(Value::UNDEFINED)
}

fn sandbox(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    content(agent, this, "iframe", "sandbox")?;
    token_list::list_of(agent, this, 1)
}

fn set_sandbox(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let list = sandbox(agent, this, &[])?;
    let key = agent.heap.key("value")?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let object = list.as_object().expect("a token list");
    agent.set(object, key, value, true)?;
    Ok(Value::UNDEFINED)
}
