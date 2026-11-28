//! Embedded content and hyperlinks: `HTMLImageElement` (HTML §4.8.4),
//! `HTMLIFrameElement` (§4.8.5), `HTMLAnchorElement`, `HTMLAreaElement`
//! (§4.5.1, §4.8.13) and `HTMLLinkElement` (§4.2.4), their reflected
//! content attributes (`loading`, CORS settings and referrer policies among
//! them) and the `sandbox` and `relList` token lists.
//!
//! Loading itself (srcset selection, lazy loading, sandboxed and `srcdoc`
//! frames) happens where documents are rendered. An image's current
//! request (`currentSrc`, `naturalWidth`, `complete`, `decode()`) comes from
//! the host's last image load through a layout read
//! ([`super::PageTree::image_state`]), and `width`/`height` read the rendered
//! size. `decode()` settles at once and `contentWindow` is missing (TD-218).

use rustic_dom::{DomString, NodeKind, QualifiedName};
use rustic_js::runtime::{Abrupt, Agent, JsResult, NativeFn, ObjectId, PropertyDescriptor, Value};
use rustic_url::Url;

use crate::dom_exception;
use crate::webidl::{DomStringConversion, IntegerConversion, to_dom_string, to_unsigned_long};

use super::ImageState;

use super::token_list;
use super::traversal::{node_of, read_doc};
use super::{illegal_constructor, install_interface, with_mut_tree, with_page};

pub(super) const IMAGE: &str = "HTMLImageElement";
pub(super) const IFRAME: &str = "HTMLIFrameElement";
pub(super) const ANCHOR: &str = "HTMLAnchorElement";
pub(super) const AREA: &str = "HTMLAreaElement";
pub(super) const LINK: &str = "HTMLLinkElement";

/// How a reflected attribute converts (HTML §2.6.1).
#[derive(Clone, Copy)]
enum Kind {
    Text,
    Url,
    Boolean,
    /// Limited to known values: the keywords and the missing/invalid default.
    Keywords(&'static [&'static str], &'static str),
    /// A CORS settings attribute: nullable, `anonymous` or `use-credentials`.
    Cors,
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
const REFLECTED: [(&str, &str, Kind, &str); 47] = [
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
    ("crossOrigin", "crossorigin", Kind::Cors, "img"),
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
    ("href", "href", Kind::Url, "a"),
    ("target", "target", Kind::Text, "a"),
    ("download", "download", Kind::Text, "a"),
    ("ping", "ping", Kind::Text, "a"),
    ("rel", "rel", Kind::Text, "a"),
    ("referrerPolicy", "referrerpolicy", REFERRER, "a"),
    ("hreflang", "hreflang", Kind::Text, "a"),
    ("type", "type", Kind::Text, "a"),
    ("alt", "alt", Kind::Text, "area"),
    ("coords", "coords", Kind::Text, "area"),
    ("shape", "shape", Kind::Text, "area"),
    ("href", "href", Kind::Url, "area"),
    ("target", "target", Kind::Text, "area"),
    ("download", "download", Kind::Text, "area"),
    ("ping", "ping", Kind::Text, "area"),
    ("rel", "rel", Kind::Text, "area"),
    ("referrerPolicy", "referrerpolicy", REFERRER, "area"),
    ("href", "href", Kind::Url, "link"),
    ("crossOrigin", "crossorigin", Kind::Cors, "link"),
    ("rel", "rel", Kind::Text, "link"),
    ("as", "as", Kind::Text, "link"),
    ("media", "media", Kind::Text, "link"),
    ("integrity", "integrity", Kind::Text, "link"),
    ("hreflang", "hreflang", Kind::Text, "link"),
    ("type", "type", Kind::Text, "link"),
    ("referrerPolicy", "referrerpolicy", REFERRER, "link"),
    ("disabled", "disabled", Kind::Boolean, "link"),
    (
        "fetchPriority",
        "fetchpriority",
        Kind::Keywords(&["high", "low", "auto"], "auto"),
        "link",
    ),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        IMAGE,
        Some("HTMLElement"),
        illegal_constructor,
        0,
        &[],
        |agent, proto| {
            define_reflected(agent, proto, "img")?;
            for (name, get, set) in [
                (
                    "width",
                    image_width::<0> as NativeFn,
                    Some(set_dimension::<0> as NativeFn),
                ),
                ("height", image_width::<1>, Some(set_dimension::<1>)),
                ("naturalWidth", natural::<0>, None),
                ("naturalHeight", natural::<1>, None),
                ("complete", complete, None),
                ("currentSrc", current_src, None),
            ] {
                accessor(agent, proto, name, get, set)?;
            }
            let key = agent.heap.key("decode")?;
            agent.define_method(proto, key, decode, "decode", 0)
        },
    )?;
    for (interface, local) in [(ANCHOR, "a"), (AREA, "area"), (LINK, "link")] {
        install_interface(
            agent,
            interface,
            Some("HTMLElement"),
            illegal_constructor,
            0,
            &[],
            |agent, proto| {
                define_reflected(agent, proto, local)?;
                token_list::define_list_accessor(agent, proto, "relList", rel_list, set_rel_list)
            },
        )?;
    }
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
const GETTERS: [NativeFn; 47] = [
    get::<0>, get::<1>, get::<2>, get::<3>, get::<4>, get::<5>, get::<6>, get::<7>, get::<8>,
    get::<9>, get::<10>, get::<11>, get::<12>, get::<13>, get::<14>, get::<15>, get::<16>,
    get::<17>, get::<18>, get::<19>, get::<20>, get::<21>, get::<22>, get::<23>, get::<24>,
    get::<25>, get::<26>, get::<27>, get::<28>, get::<29>, get::<30>, get::<31>, get::<32>,
    get::<33>, get::<34>, get::<35>, get::<36>, get::<37>, get::<38>, get::<39>, get::<40>,
    get::<41>, get::<42>, get::<43>, get::<44>, get::<45>, get::<46>,
];
const SETTERS: [NativeFn; 47] = [
    set::<0>, set::<1>, set::<2>, set::<3>, set::<4>, set::<5>, set::<6>, set::<7>, set::<8>,
    set::<9>, set::<10>, set::<11>, set::<12>, set::<13>, set::<14>, set::<15>, set::<16>,
    set::<17>, set::<18>, set::<19>, set::<20>, set::<21>, set::<22>, set::<23>, set::<24>,
    set::<25>, set::<26>, set::<27>, set::<28>, set::<29>, set::<30>, set::<31>, set::<32>,
    set::<33>, set::<34>, set::<35>, set::<36>, set::<37>, set::<38>, set::<39>, set::<40>,
    set::<41>, set::<42>, set::<43>, set::<44>, set::<45>, set::<46>,
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
        Kind::Cors => match value {
            None => Ok(Value::NULL),
            Some(value) if value.eq_ignore_ascii_case("use-credentials") => {
                agent.string("use-credentials")
            }
            Some(_) => agent.string("anonymous"),
        },
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
        Kind::Cors if arg == Value::NULL => None,
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

fn accessor(
    agent: &mut Agent,
    proto: ObjectId,
    name: &str,
    get: NativeFn,
    set: Option<NativeFn>,
) -> JsResult<()> {
    let get = Value::object(agent.new_native_function(get, &format!("get {name}"), 0)?);
    let set = match set {
        Some(set) => Value::object(agent.new_native_function(set, &format!("set {name}"), 1)?),
        None => Value::UNDEFINED,
    };
    let key = agent.heap.key(name)?;
    agent.define_property_or_throw(
        proto,
        key,
        PropertyDescriptor::accessor(get, set, true, true),
    )
}

fn rel_list(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let id = node_of(agent, this)?;
    let local = read_doc(agent, id, |d| match d.node(id).map(|n| n.kind()) {
        Ok(NodeKind::Element(e)) if e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE) => {
            Some(e.name().local().to_owned())
        }
        _ => None,
    })?;
    if !matches!(local.as_deref(), Some("a" | "area" | "link")) {
        return agent.type_error("expected an HTML a, area or link element receiver");
    }
    token_list::list_of(agent, this, 2)
}

fn set_rel_list(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let list = rel_list(agent, this, &[])?;
    let key = agent.heap.key("value")?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let object = list.as_object().expect("a token list");
    agent.set(object, key, value, true)?;
    Ok(Value::UNDEFINED)
}

// ---- image request state -----------------------------------------------------

/// The current request of the `img` behind `this` after the host's last
/// image load (a layout read; `None` while disconnected or not requested).
fn image_state(agent: &mut Agent, this: Value) -> JsResult<Option<ImageState>> {
    content(agent, this, "img", "src")?;
    super::read_layout(agent, this, true, None, |tree, id| tree.image_state(id))
}

/// `naturalWidth`/`naturalHeight`: the density-corrected natural size, or
/// 0 without an available image.
fn natural<const AXIS: usize>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let size = image_state(agent, this)?.and_then(|state| state.natural_size);
    Ok(Value::number(size.map_or(0.0, |size| size[AXIS].trunc())))
}

/// `width`/`height`: the rendered content size when the image has a box,
/// else its natural size, else 0 (HTML §4.8.4.4).
fn image_width<const AXIS: usize>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    content(agent, this, "img", "src")?;
    let rendered = super::read_layout(agent, this, true, None, |tree, id| tree.box_sizes(id))?;
    if let Some(sizes) = rendered {
        return Ok(Value::number(sizes.content[AXIS].max(0.0).trunc()));
    }
    natural::<AXIS>(agent, this, &[])
}

fn set_dimension<const AXIS: usize>(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
) -> JsResult<Value> {
    content(agent, this, "img", "src")?;
    let arg = args.first().copied().unwrap_or(Value::UNDEFINED);
    let value = to_unsigned_long(agent, arg, IntegerConversion::Wrap)?;
    // Values outside the non-negative long range reflect as 0.
    let value = if value > i32::MAX as u32 { 0 } else { value };
    let name = ["width", "height"][AXIS];
    with_mut_tree(agent, this, |agent, document, _, id| {
        let qualified = QualifiedName::from_parsed(None, None, name)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        document
            .set_attribute(id, qualified, DomString::from(value.to_string().as_str()))
            .map(|_| ())
            .map_err(|error| dom_exception::from_dom_error(agent, error))
    })?;
    Ok(Value::UNDEFINED)
}

/// `complete`: no source to request, or the current request finished
/// (decoded or broken).
fn complete(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let srcset = content(agent, this, "img", "srcset")?;
    let src = content(agent, this, "img", "src")?;
    if srcset.is_none() && src.as_deref().is_none_or(str::is_empty) {
        return Ok(Value::boolean(true));
    }
    Ok(Value::boolean(image_state(agent, this)?.is_some()))
}

fn current_src(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let state = image_state(agent, this)?;
    agent.string(
        state
            .as_ref()
            .map_or("", |state| state.current_src.as_str()),
    )
}

/// `decode()`: resolves once the current request decoded; rejects with an
/// `EncodingError` when it is broken or was not requested (TD-218).
fn decode(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    content(agent, this, "img", "src")?;
    let promise = agent.new_host_promise()?;
    let result = match image_state(agent, this) {
        Ok(Some(ImageState {
            natural_size: Some(_),
            ..
        })) => Ok(()),
        Ok(_) => Err(dom_exception::new(
            agent,
            "EncodingError",
            "the image could not be decoded",
        )?),
        Err(Abrupt::Throw(error)) => Err(error),
        Err(error) => return Err(error),
    };
    match result {
        Ok(()) => agent.resolve_host_promise(promise, Value::UNDEFINED)?,
        Err(reason) => agent.reject_host_promise(promise, reason),
    }
    Ok(Value::object(promise))
}
