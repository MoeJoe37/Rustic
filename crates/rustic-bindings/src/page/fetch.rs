//! Fetch Standard §5 script interfaces: `Headers`, `Request`, `Response`
//! and `fetch()`.
//!
//! The network is the embedder's: [`super::attach_fetch`] supplies a
//! [`FetchHost`] that resolves URLs against the document and runs the
//! request under the document's policies. Without one, `fetch()` rejects
//! with a `TypeError` and relative URLs cannot be parsed.
//!
//! State lives in prototype-less private arrays in a weak map, keyed by
//! the script object. Header lists are byte strings stored as Latin-1 JS
//! strings. Bodies are complete byte strings: `text()`, `json()` and
//! `bodyUsed`, `arrayBuffer()`, `blob()` and `bytes()` work. Streams and
//! FormData remain part of TD-169.

use std::borrow::Cow;
use std::fmt::Debug;
use std::rc::Rc;

use rustic_js::runtime::{
    Abrupt, Agent, ErrorKind, JsResult, NativeFn, ObjectId, PropertyKey, Value, to_uint16,
};
use rustic_net::cors::{
    is_cors_safelisted_method, is_forbidden_method, is_forbidden_request_header,
    is_forbidden_response_header_name, is_no_cors_safelisted_request_header, normalize_method,
};
use rustic_url::Url;

use super::events::{PrivateArray, define_method, with_root};
use super::{illegal_constructor, install_interface, prototype, required_arg, with_page};
use crate::webidl::{from_dom_units, to_byte_string, to_usv_string};

/// A request as `fetch()` hands it to the host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptRequest {
    /// The normalized method.
    pub method: String,
    /// The parsed URL.
    pub url: Url,
    /// The header list in order.
    pub headers: Vec<(String, Vec<u8>)>,
    /// The body bytes, if any.
    pub body: Option<Vec<u8>>,
    /// `same-origin`, `no-cors` or `cors`.
    pub mode: String,
    /// `omit`, `same-origin` or `include`.
    pub credentials: String,
    /// `follow`, `error` or `manual`.
    pub redirect: String,
    /// A referrer policy token, or empty for the document's.
    pub referrer_policy: String,
}

/// A filtered response from the host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptResponse {
    /// `basic`, `cors`, `opaque` or `opaqueredirect`.
    pub kind: &'static str,
    /// The final URL, if visible.
    pub url: Option<Url>,
    /// Whether redirects were followed.
    pub redirected: bool,
    /// The status (0 for opaque responses).
    pub status: u16,
    /// The status message.
    pub status_text: Vec<u8>,
    /// The visible header list.
    pub headers: Vec<(String, Vec<u8>)>,
    /// The visible body.
    pub body: Vec<u8>,
}

/// A network error: script sees only a `TypeError`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkFailure;

/// The embedder's network for one document. `fetch` may block; it must
/// not call back into script.
pub trait FetchHost: Debug {
    /// The document's API base URL.
    fn base_url(&self) -> Option<Url>;
    /// Run the request; `Err` is a network error (`TypeError` in script).
    fn fetch(&self, request: ScriptRequest) -> Result<ScriptResponse, NetworkFailure>;
}

const HEADERS: &str = "Headers";
const REQUEST: &str = "Request";
const RESPONSE: &str = "Response";
const BODY_TEXT: u8 = 0;
const BODY_JSON: u8 = 1;
const BODY_BUFFER: u8 = 2;
const BODY_BYTES: u8 = 3;
const BODY_BLOB: u8 = 4;

// Header guards (Fetch §5.1).
const GUARD_NONE: f64 = 0.0;
const GUARD_REQUEST: f64 = 1.0;
const GUARD_NO_CORS: f64 = 2.0;
const GUARD_RESPONSE: f64 = 3.0;
const GUARD_IMMUTABLE: f64 = 4.0;

// Record layouts: slot 0 is the interface tag.
const TAG: u32 = 0;
// Headers: [tag, guard, name0, value0, name1, value1, ...]
const H_GUARD: u32 = 1;
const H_LIST: u32 = 2;
// Request: [tag, method, url, headers, mode, credentials, redirect,
//           referrerPolicy, body or null, bodyUsed]
// Response: [tag, type, url, status, statusText, headers, body or null,
//            bodyUsed, redirected]
const R_HEADERS_REQUEST: u32 = 3;
const R_BODY_REQUEST: u32 = 8;
const R_USED_REQUEST: u32 = 9;
const R_HEADERS_RESPONSE: u32 = 5;
const R_BODY_RESPONSE: u32 = 6;
const R_USED_RESPONSE: u32 = 7;

const HEADERS_ITERATOR: &str = "Headers Iterator";

/// Install the interfaces; returns the `Headers Iterator` prototype, which
/// the page keeps.
pub(super) fn install(agent: &mut Agent) -> JsResult<ObjectId> {
    install_interface(
        agent,
        HEADERS,
        None,
        construct_headers,
        0,
        &[],
        |agent, proto| {
            for (name, method, length) in [
                ("append", headers_append as NativeFn, 2),
                ("delete", headers_delete, 1),
                ("get", headers_get, 1),
                ("getSetCookie", headers_get_set_cookie, 0),
                ("has", headers_has, 1),
                ("set", headers_set, 2),
                ("forEach", headers_for_each, 1),
                ("entries", headers_entries::<0>, 0),
                ("keys", headers_entries::<1>, 0),
                ("values", headers_entries::<2>, 0),
            ] {
                define_method(agent, proto, name, method, length)?;
            }
            let entries_key = agent.heap.key("entries")?;
            let entries = agent.get(proto, entries_key)?;
            agent.define_property_or_throw(
                proto,
                PropertyKey::Symbol(agent.symbols.iterator),
                rustic_js::runtime::PropertyDescriptor::data_with(entries, true, false, true),
            )?;
            Ok(())
        },
    )?;
    install_interface(
        agent,
        REQUEST,
        None,
        construct_request,
        1,
        &[
            ("method", "get method", request_get::<1>),
            ("url", "get url", request_get::<2>),
            ("headers", "get headers", request_get::<3>),
            ("mode", "get mode", request_get::<4>),
            ("credentials", "get credentials", request_get::<5>),
            ("redirect", "get redirect", request_get::<6>),
            ("referrerPolicy", "get referrerPolicy", request_get::<7>),
            ("bodyUsed", "get bodyUsed", request_get::<9>),
            ("destination", "get destination", request_destination),
            ("referrer", "get referrer", request_constant::<0>),
            ("cache", "get cache", request_constant::<1>),
            ("integrity", "get integrity", request_constant::<2>),
            (
                "isReloadNavigation",
                "get isReloadNavigation",
                request_constant::<3>,
            ),
            (
                "isHistoryNavigation",
                "get isHistoryNavigation",
                request_constant::<3>,
            ),
            ("duplex", "get duplex", request_constant::<4>),
            ("keepalive", "get keepalive", request_constant::<3>),
        ],
        |agent, proto| {
            install_body_methods::<false>(agent, proto)?;
            define_method(agent, proto, "clone", request_clone, 0)
        },
    )?;
    install_interface(
        agent,
        RESPONSE,
        None,
        construct_response,
        0,
        &[
            ("type", "get type", response_get::<1>),
            ("url", "get url", response_get::<2>),
            ("status", "get status", response_get::<3>),
            ("ok", "get ok", response_ok),
            ("statusText", "get statusText", response_get::<4>),
            ("headers", "get headers", response_get::<5>),
            ("bodyUsed", "get bodyUsed", response_get::<7>),
            ("redirected", "get redirected", response_get::<8>),
        ],
        |agent, proto| {
            install_body_methods::<true>(agent, proto)?;
            define_method(agent, proto, "clone", response_clone, 0)
        },
    )?;
    let response_prototype = prototype(agent, RESPONSE)?;
    let constructor_key = PropertyKey::String(agent.names.constructor);
    let response = agent.get(response_prototype, constructor_key)?;
    if let Some(constructor) = response.as_object() {
        define_method(agent, constructor, "error", response_error, 0)?;
        define_method(agent, constructor, "json", response_json, 1)?;
    }
    let global = agent.intrinsics.global;
    define_method(agent, global, "fetch", fetch, 1)?;
    let _ = illegal_constructor;
    // Web IDL §3.7.10.1: the pair iterator prototype inherits from
    // %IteratorPrototype% and has an enumerable `next`.
    let array_iterator = agent.intrinsics.array_iterator_prototype;
    let iterator_prototype = agent.get_prototype_of(array_iterator)?;
    let prototype = agent.new_object_with(iterator_prototype)?;
    define_method(agent, prototype, "next", headers_iterator_next, 0)?;
    let tag = Value::string(agent.heap.intern_str(HEADERS_ITERATOR)?);
    agent.define_property_or_throw(
        prototype,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        rustic_js::runtime::PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    Ok(prototype)
}

// ---- records ------------------------------------------------------------

fn bytes_value(agent: &mut Agent, bytes: &[u8]) -> JsResult<Value> {
    let units: Vec<u16> = bytes.iter().map(|&byte| u16::from(byte)).collect();
    from_dom_units(agent, &units)
}

fn value_bytes(agent: &Agent, value: Value) -> Vec<u8> {
    match value.as_string() {
        Some(id) => agent
            .heap
            .string(id)
            .units()
            .map(|unit| unit as u8)
            .collect(),
        None => Vec::new(),
    }
}

fn text_value(agent: &mut Agent, text: &str) -> JsResult<Value> {
    Ok(Value::string(agent.heap.alloc_str(text)?))
}

fn value_text(agent: &Agent, value: Value) -> String {
    value
        .as_string()
        .map(|id| agent.heap.string(id).to_text())
        .unwrap_or_default()
}

fn new_record(agent: &mut Agent, object: Value, values: &[Value]) -> JsResult<PrivateArray> {
    let record = PrivateArray::new(agent, values)?;
    with_page(agent, |agent, page| {
        agent.weak_map_set(page.fetch_records, object, record.value())
    })?;
    Ok(record)
}

fn record(agent: &mut Agent, object: Value, interface: &str) -> JsResult<PrivateArray> {
    let found = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.fetch_records, object))
    })?;
    if let Some(record) = PrivateArray::from_value(found) {
        if {
            let read = record.get(agent, TAG)?;
            value_text(agent, read)
        } == interface
        {
            return Ok(record);
        }
    }
    agent.type_error(&format!("expected a {interface} receiver"))
}

fn type_error(agent: &mut Agent, message: &str) -> Abrupt {
    agent.throw(ErrorKind::TypeError, message)
}

// ---- header lists ---------------------------------------------------------

type HeaderList = Vec<(String, Vec<u8>)>;

fn read_list(agent: &mut Agent, record: PrivateArray) -> JsResult<(f64, HeaderList)> {
    let guard = record
        .get(agent, H_GUARD)?
        .as_number()
        .unwrap_or(GUARD_NONE);
    let length = record.len(agent)?;
    let mut list = Vec::new();
    let mut index = H_LIST;
    while index + 1 < length {
        let name = {
            let read = record.get(agent, index)?;
            value_text(agent, read)
        };
        let value = {
            let read = record.get(agent, index + 1)?;
            value_bytes(agent, read)
        };
        list.push((name, value));
        index += 2;
    }
    Ok((guard, list))
}

/// Make a `Headers` object holding `list` under `guard`.
fn new_headers(agent: &mut Agent, guard: f64, list: &HeaderList) -> JsResult<Value> {
    let proto = prototype(agent, HEADERS)?;
    let object = Value::object(agent.new_object_with(Some(proto))?);
    let values = list_values(agent, guard, list)?;
    new_record(agent, object, &values)?;
    Ok(object)
}

fn list_values(agent: &mut Agent, guard: f64, list: &HeaderList) -> JsResult<Vec<Value>> {
    let mut values = vec![text_value(agent, HEADERS)?, Value::number(guard)];
    for (name, value) in list {
        values.push(text_value(agent, name)?);
        values.push(bytes_value(agent, value)?);
    }
    Ok(values)
}

fn write_list(agent: &mut Agent, object: Value, guard: f64, list: &HeaderList) -> JsResult<()> {
    let values = list_values(agent, guard, list)?;
    new_record(agent, object, &values)?;
    Ok(())
}

/// Fetch "normalize" a header value: strip leading and trailing HTTP
/// whitespace.
fn normalize_value(value: &[u8]) -> Vec<u8> {
    let is_space = |byte: &u8| matches!(byte, b'\t' | b'\n' | b'\r' | b' ');
    let start = value
        .iter()
        .position(|byte| !is_space(byte))
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|byte| !is_space(byte))
        .map_or(start, |end| end + 1);
    value[start..end].to_vec()
}

fn validate(agent: &mut Agent, name: &[u8], value: &[u8]) -> JsResult<String> {
    if !rustic_net::is_token(name) {
        return Err(type_error(agent, "invalid header name"));
    }
    if !rustic_net::is_valid_header_value(value) {
        return Err(type_error(agent, "invalid header value"));
    }
    Ok(String::from_utf8_lossy(name).into_owned())
}

/// Whether the guard silently ignores this change (Fetch §5.1 "append"
/// and "set" steps 2–5). An immutable guard throws.
fn guard_ignores(agent: &mut Agent, guard: f64, name: &str, value: &[u8]) -> JsResult<bool> {
    if guard == GUARD_IMMUTABLE {
        return Err(type_error(agent, "headers are immutable"));
    }
    Ok((guard == GUARD_REQUEST || guard == GUARD_NO_CORS)
        && is_forbidden_request_header(name, value)
        || guard == GUARD_RESPONSE && is_forbidden_response_header_name(name))
}

fn combined(list: &HeaderList, name: &str) -> Option<Vec<u8>> {
    let mut out: Option<Vec<u8>> = None;
    for (key, value) in list {
        if key.eq_ignore_ascii_case(name) {
            match &mut out {
                Some(out) => {
                    out.extend_from_slice(b", ");
                    out.extend_from_slice(value);
                }
                None => out = Some(value.clone()),
            }
        }
    }
    out
}

fn append(
    agent: &mut Agent,
    guard: f64,
    list: &mut HeaderList,
    name: &[u8],
    value: &[u8],
) -> JsResult<()> {
    let value = normalize_value(value);
    let name = validate(agent, name, &value)?;
    if guard_ignores(agent, guard, &name, &value)? {
        return Ok(());
    }
    if guard == GUARD_NO_CORS {
        let mut temporary = combined(list, &name).unwrap_or_default();
        if !temporary.is_empty() {
            temporary.extend_from_slice(b", ");
        }
        temporary.extend_from_slice(&value);
        if !is_no_cors_safelisted_request_header(&name, &temporary) {
            return Ok(());
        }
    }
    list.push((name, value));
    Ok(())
}

/// Fetch "sort and combine": lowercase names in order, `set-cookie`
/// values kept apart.
fn sort_and_combine(list: &HeaderList) -> Vec<(String, Vec<u8>)> {
    let mut names: Vec<String> = list
        .iter()
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect();
    names.sort();
    names.dedup();
    let mut out = Vec::new();
    for name in names {
        if name == "set-cookie" {
            for (key, value) in list {
                if key.eq_ignore_ascii_case(&name) {
                    out.push((name.clone(), value.clone()));
                }
            }
        } else if let Some(value) = combined(list, &name) {
            out.push((name, value));
        }
    }
    out
}

/// Fill a header list from a `HeadersInit` (sequence of pairs or record).
fn fill(agent: &mut Agent, guard: f64, list: &mut HeaderList, init: Value) -> JsResult<()> {
    let Some(object) = init.as_object() else {
        return Err(type_error(agent, "headers init must be an object"));
    };
    let iterator = agent.get_method(init, PropertyKey::Symbol(agent.symbols.iterator))?;
    let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    if iterator.is_some() {
        with_root(agent, init, |agent| {
            agent.iterate_values(init, |agent, item| {
                let mut pair = Vec::new();
                with_root(agent, item, |agent| {
                    agent.iterate_values(item, |agent, part| {
                        pair.push(to_byte_string(agent, part)?);
                        Ok(true)
                    })
                })?;
                let [name, value]: [Vec<u8>; 2] = pair
                    .try_into()
                    .map_err(|_| type_error(agent, "a header pair must have two items"))?;
                pairs.push((name, value));
                Ok(true)
            })
        })?;
    } else {
        with_root(agent, init, |agent| {
            for key in agent.own_property_keys(object)? {
                let Some(desc) = agent.get_own_property(object, key)? else {
                    continue;
                };
                if desc.enumerable != Some(true) {
                    continue;
                }
                if matches!(key, PropertyKey::Symbol(_)) {
                    return Err(type_error(agent, "a symbol cannot be a header name"));
                }
                let name = agent.key_to_value(key)?;
                let name = to_byte_string(agent, name)?;
                let value = agent.get(object, key)?;
                let value = to_byte_string(agent, value)?;
                pairs.push((name, value));
            }
            Ok(())
        })?;
    }
    for (name, value) in pairs {
        append(agent, guard, list, &name, &value)?;
    }
    Ok(())
}

// ---- Headers ----------------------------------------------------------------

fn construct_headers(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let mut list = Vec::new();
    if let Some(&init) = args.first().filter(|&&init| init != Value::UNDEFINED) {
        fill(agent, GUARD_NONE, &mut list, init)?;
    }
    new_headers(agent, GUARD_NONE, &list)
}

fn name_arg(agent: &mut Agent, args: &[Value]) -> JsResult<String> {
    let name = required_arg(agent, args, 0)?;
    let name = to_byte_string(agent, name)?;
    if !rustic_net::is_token(&name) {
        return Err(type_error(agent, "invalid header name"));
    }
    Ok(String::from_utf8_lossy(&name).into_owned())
}

fn headers_append(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = record(agent, this, HEADERS)?;
    let name = required_arg(agent, args, 0)?;
    let value = required_arg(agent, args, 1)?;
    let name = to_byte_string(agent, name)?;
    let value = to_byte_string(agent, value)?;
    let (guard, mut list) = read_list(agent, record)?;
    append(agent, guard, &mut list, &name, &value)?;
    write_list(agent, this, guard, &list)?;
    Ok(Value::UNDEFINED)
}

fn headers_delete(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = record(agent, this, HEADERS)?;
    let name = name_arg(agent, args)?;
    let (guard, mut list) = read_list(agent, record)?;
    if guard_ignores(agent, guard, &name, b"")? {
        return Ok(Value::UNDEFINED);
    }
    if guard == GUARD_NO_CORS
        && !is_no_cors_safelisted_request_header(&name, b"text/plain")
        && !name.eq_ignore_ascii_case("range")
    {
        return Ok(Value::UNDEFINED);
    }
    list.retain(|(key, _)| !key.eq_ignore_ascii_case(&name));
    write_list(agent, this, guard, &list)?;
    Ok(Value::UNDEFINED)
}

fn headers_get(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = record(agent, this, HEADERS)?;
    let name = name_arg(agent, args)?;
    let (_, list) = read_list(agent, record)?;
    match combined(&list, &name) {
        Some(value) => bytes_value(agent, &value),
        None => Ok(Value::NULL),
    }
}

fn headers_get_set_cookie(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let record = record(agent, this, HEADERS)?;
    let (_, list) = read_list(agent, record)?;
    let mut values = Vec::new();
    for (name, value) in &list {
        if name.eq_ignore_ascii_case("set-cookie") {
            values.push(bytes_value(agent, value)?);
        }
    }
    Ok(Value::object(agent.create_array_from_list(&values)?))
}

fn headers_has(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = record(agent, this, HEADERS)?;
    let name = name_arg(agent, args)?;
    let (_, list) = read_list(agent, record)?;
    Ok(Value::boolean(
        list.iter().any(|(key, _)| key.eq_ignore_ascii_case(&name)),
    ))
}

fn headers_set(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = record(agent, this, HEADERS)?;
    let name = required_arg(agent, args, 0)?;
    let value = required_arg(agent, args, 1)?;
    let name = to_byte_string(agent, name)?;
    let value = normalize_value(&to_byte_string(agent, value)?);
    let name = validate(agent, &name, &value)?;
    let (guard, mut list) = read_list(agent, record)?;
    if guard_ignores(agent, guard, &name, &value)?
        || guard == GUARD_NO_CORS && !is_no_cors_safelisted_request_header(&name, &value)
    {
        return Ok(Value::UNDEFINED);
    }
    match list
        .iter()
        .position(|(key, _)| key.eq_ignore_ascii_case(&name))
    {
        Some(first) => {
            list[first].1 = value;
            let mut index = 0;
            list.retain(|(key, _)| {
                index += 1;
                index - 1 == first || !key.eq_ignore_ascii_case(&name)
            });
        }
        None => list.push((name, value)),
    }
    write_list(agent, this, guard, &list)?;
    Ok(Value::UNDEFINED)
}

fn headers_for_each(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = record(agent, this, HEADERS)?;
    let callback = required_arg(agent, args, 0)?;
    if !agent.is_callable(callback) {
        return Err(type_error(agent, "forEach callback is not callable"));
    }
    let this_arg = args.get(1).copied().unwrap_or(Value::UNDEFINED);
    let (_, list) = read_list(agent, record)?;
    for (name, value) in sort_and_combine(&list) {
        let name = text_value(agent, &name)?;
        let value = bytes_value(agent, &value)?;
        agent.call(callback, this_arg, &[value, name, this])?;
    }
    Ok(Value::UNDEFINED)
}

/// `entries()`, `keys()` and `values()`: a live pair iterator whose index
/// walks the current sorted and combined list on every `next()`.
fn headers_entries<const KIND: u8>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    record(agent, this, HEADERS)?;
    let prototype = with_page(agent, |_, page| Ok(page.headers_iterator))?;
    let iterator = Value::object(agent.new_object_with(Some(prototype))?);
    let tag = text_value(agent, HEADERS_ITERATOR)?;
    new_record(
        agent,
        iterator,
        &[
            tag,
            this,
            Value::number(f64::from(KIND)),
            Value::number(0.0),
        ],
    )?;
    Ok(iterator)
}

fn headers_iterator_next(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let state = record(agent, this, HEADERS_ITERATOR)?;
    let headers = state.get(agent, 1)?;
    let kind = state.get(agent, 2)?.as_number().unwrap_or(0.0);
    let index = state.get(agent, 3)?.as_number().unwrap_or(0.0) as usize;
    let list = record(agent, headers, HEADERS)?;
    let (_, list) = read_list(agent, list)?;
    let pairs = sort_and_combine(&list);
    let result = agent.new_object()?;
    let value_key = PropertyKey::String(agent.names.value);
    let done_key = PropertyKey::String(agent.names.done);
    let Some((name, value)) = pairs.get(index) else {
        agent.create_data_property_or_throw(result, value_key, Value::UNDEFINED)?;
        agent.create_data_property_or_throw(result, done_key, Value::TRUE)?;
        return Ok(Value::object(result));
    };
    state.set(agent, 3, Value::number((index + 1) as f64))?;
    let name = text_value(agent, name)?;
    let value = bytes_value(agent, value)?;
    let item = if kind == 0.0 {
        Value::object(agent.create_array_from_list(&[name, value])?)
    } else if kind == 1.0 {
        name
    } else {
        value
    };
    agent.create_data_property_or_throw(result, value_key, item)?;
    agent.create_data_property_or_throw(result, done_key, Value::FALSE)?;
    Ok(Value::object(result))
}

// ---- dictionaries ---------------------------------------------------------

fn member(agent: &mut Agent, init: Value, name: &str) -> JsResult<Value> {
    match init.as_object() {
        Some(object) => {
            let key = agent.heap.key(name)?;
            agent.get(object, key)
        }
        None => Ok(Value::UNDEFINED),
    }
}

fn enum_member(
    agent: &mut Agent,
    init: Value,
    name: &str,
    allowed: &[&str],
) -> JsResult<Option<String>> {
    let value = member(agent, init, name)?;
    if value == Value::UNDEFINED {
        return Ok(None);
    }
    let text = to_usv_string(agent, value)?;
    if !allowed.contains(&text.as_str()) {
        return Err(type_error(agent, &format!("invalid {name} value")));
    }
    Ok(Some(text))
}

/// IDL conversion happens before body extraction. An options getter may
/// detach a buffer or mutate the search parameters before the byte copy.
enum BodyInit {
    Empty,
    Text(String),
    Bytes(ObjectId),
    SearchParams(Value),
    Blob(Rc<super::blob::Data>),
}

struct ExtractedBody {
    bytes: Vec<u8>,
    content_type: Option<Cow<'static, [u8]>>,
}

impl BodyInit {
    fn convert(agent: &mut Agent, body: Value) -> JsResult<Self> {
        if body.is_nullish() {
            return Ok(Self::Empty);
        }
        if agent.is_web_buffer_source(body) {
            return agent.web_unshared_buffer_source(body).map(Self::Bytes);
        }
        if super::url::is_search_params(agent, body) {
            return Ok(Self::SearchParams(body));
        }
        if let Some(blob) = super::blob::find(agent, body) {
            return Ok(Self::Blob(blob));
        }
        to_usv_string(agent, body).map(Self::Text)
    }

    fn extract(self, agent: &mut Agent) -> JsResult<Option<ExtractedBody>> {
        let (bytes, content_type) = match self {
            Self::Empty => return Ok(None),
            Self::Text(text) => (
                text.into_bytes(),
                Some(Cow::Borrowed(b"text/plain;charset=UTF-8".as_slice())),
            ),
            Self::Bytes(source) => {
                let length = agent.web_buffer_bytes(source).len();
                let copy = agent.with_host_memory(length, |agent| {
                    let mut copy = Vec::new();
                    copy.try_reserve_exact(length)
                        .map_err(|_| Abrupt::OutOfMemory)?;
                    copy.extend_from_slice(&agent.web_buffer_bytes(source));
                    Ok(copy)
                })?;
                (copy, None)
            }
            Self::SearchParams(value) => (
                super::url::serialize_search_params(agent, value)?.into_bytes(),
                Some(Cow::Borrowed(
                    b"application/x-www-form-urlencoded;charset=UTF-8".as_slice(),
                )),
            ),
            Self::Blob(blob) => {
                let bytes = agent.with_host_memory(blob.bytes.len(), |_| {
                    let mut bytes = Vec::new();
                    bytes
                        .try_reserve_exact(blob.bytes.len())
                        .map_err(|_| Abrupt::OutOfMemory)?;
                    bytes.extend_from_slice(&blob.bytes);
                    Ok(bytes)
                })?;
                let content_type = (!blob.media_type.is_empty())
                    .then(|| Cow::Owned(blob.media_type.as_bytes().to_vec()));
                (bytes, content_type)
            }
        };
        Ok(Some(ExtractedBody {
            bytes,
            content_type,
        }))
    }
}

fn append_body_content_type(list: &mut HeaderList, content_type: Option<&[u8]>) {
    if let Some(content_type) = content_type {
        if !list
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        {
            list.push(("Content-Type".to_owned(), content_type.to_vec()));
        }
    }
}

// ---- Request ----------------------------------------------------------------

struct RequestState {
    method: String,
    url: Url,
    headers: HeaderList,
    mode: String,
    credentials: String,
    redirect: String,
    referrer_policy: String,
    body: Option<Vec<u8>>,
}

fn parse_url(agent: &mut Agent, input: &str) -> JsResult<Url> {
    let base = with_page(agent, |_, page| {
        Ok(page
            .fetch
            .borrow()
            .as_ref()
            .and_then(|host| host.base_url()))
    })?;
    match Url::parse(input, base.as_ref()) {
        Ok(url) if url.username().is_empty() && url.password().is_empty() => Ok(url),
        Ok(_) => Err(type_error(agent, "URLs with credentials cannot be fetched")),
        Err(_) => Err(type_error(agent, "invalid URL")),
    }
}

fn read_request(agent: &mut Agent, record: PrivateArray) -> JsResult<RequestState> {
    let text = |agent: &mut Agent, index| -> JsResult<String> {
        let value = record.get(agent, index)?;
        Ok(value_text(agent, value))
    };
    let url = text(agent, 2)?;
    let headers_object = record.get(agent, R_HEADERS_REQUEST)?;
    let headers_record = self::record(agent, headers_object, HEADERS)?;
    let (_, headers) = read_list(agent, headers_record)?;
    let body = record.get(agent, R_BODY_REQUEST)?;
    Ok(RequestState {
        method: text(agent, 1)?,
        url: Url::parse(&url, None).map_err(|_| type_error(agent, "invalid URL"))?,
        headers,
        mode: text(agent, 4)?,
        credentials: text(agent, 5)?,
        redirect: text(agent, 6)?,
        referrer_policy: text(agent, 7)?,
        body: (!(body == Value::NULL)).then(|| value_bytes(agent, body)),
    })
}

/// The `Request` constructor's steps (Fetch §5.4), without the object.
fn request_state(agent: &mut Agent, input: Value, init: Value) -> JsResult<RequestState> {
    let mut state = match input.as_object() {
        Some(_) => {
            let record = record(agent, input, REQUEST)?;
            if record.get(agent, R_USED_REQUEST)? == Value::TRUE {
                return Err(type_error(agent, "the request body is already used"));
            }
            read_request(agent, record)?
        }
        None => {
            let text = to_usv_string(agent, input)?;
            RequestState {
                method: "GET".to_owned(),
                url: parse_url(agent, &text)?,
                headers: Vec::new(),
                mode: "cors".to_owned(),
                credentials: "same-origin".to_owned(),
                redirect: "follow".to_owned(),
                referrer_policy: String::new(),
                body: None,
            }
        }
    };
    if init.is_nullish() {
        return Ok(state);
    }
    with_root(agent, init, |agent| {
        let method = member(agent, init, "method")?;
        if !(method == Value::UNDEFINED) {
            let method = to_byte_string(agent, method)?;
            if !rustic_net::is_token(&method) {
                return Err(type_error(agent, "invalid method"));
            }
            let method = String::from_utf8_lossy(&method).into_owned();
            if is_forbidden_method(&method) {
                return Err(type_error(agent, "forbidden method"));
            }
            state.method = normalize_method(&method);
        }
        if let Some(mode) = enum_member(
            agent,
            init,
            "mode",
            &["navigate", "same-origin", "no-cors", "cors"],
        )? {
            if mode == "navigate" {
                return Err(type_error(agent, "mode navigate cannot be constructed"));
            }
            state.mode = mode;
        }
        if let Some(credentials) = enum_member(
            agent,
            init,
            "credentials",
            &["omit", "same-origin", "include"],
        )? {
            state.credentials = credentials;
        }
        if let Some(redirect) =
            enum_member(agent, init, "redirect", &["follow", "error", "manual"])?
        {
            state.redirect = redirect;
        }
        if let Some(policy) = enum_member(
            agent,
            init,
            "referrerPolicy",
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
        )? {
            state.referrer_policy = policy;
        }
        if state.mode == "no-cors" && !is_cors_safelisted_method(&state.method) {
            return Err(type_error(
                agent,
                "no-cors requests need a CORS-safelisted method",
            ));
        }
        let guard = if state.mode == "no-cors" {
            GUARD_NO_CORS
        } else {
            GUARD_REQUEST
        };
        let headers = member(agent, init, "headers")?;
        if !(headers == Value::UNDEFINED) {
            let mut list = Vec::new();
            match record_if(agent, headers, HEADERS)? {
                Some(source) => {
                    let (_, source) = read_list(agent, source)?;
                    for (name, value) in source {
                        append(agent, guard, &mut list, name.as_bytes(), &value)?;
                    }
                }
                None => fill(agent, guard, &mut list, headers)?,
            }
            state.headers = list;
        }
        let body = member(agent, init, "body")?;
        let body = BodyInit::convert(agent, body)?;
        if !matches!(body, BodyInit::Empty) {
            if matches!(state.method.as_str(), "GET" | "HEAD") {
                return Err(type_error(
                    agent,
                    "GET and HEAD requests cannot have a body",
                ));
            }
            let body = body.extract(agent)?.expect("provided Request body");
            append_body_content_type(&mut state.headers, body.content_type.as_deref());
            state.body = Some(body.bytes);
        }
        Ok(())
    })?;
    Ok(state)
}

fn record_if(agent: &mut Agent, value: Value, interface: &str) -> JsResult<Option<PrivateArray>> {
    if value.as_object().is_none() {
        return Ok(None);
    }
    let found = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.fetch_records, value))
    })?;
    match PrivateArray::from_value(found) {
        Some(record)
            if {
                let read = record.get(agent, TAG)?;
                value_text(agent, read)
            } == interface =>
        {
            Ok(Some(record))
        }
        _ => Ok(None),
    }
}

fn new_request(agent: &mut Agent, state: &RequestState) -> JsResult<Value> {
    let guard = if state.mode == "no-cors" {
        GUARD_NO_CORS
    } else {
        GUARD_REQUEST
    };
    let headers = new_headers(agent, guard, &state.headers)?;
    let proto = prototype(agent, REQUEST)?;
    let object = Value::object(agent.new_object_with(Some(proto))?);
    let body = match &state.body {
        Some(body) => bytes_value(agent, body)?,
        None => Value::NULL,
    };
    let values = [
        text_value(agent, REQUEST)?,
        text_value(agent, &state.method)?,
        text_value(agent, &state.url.serialize())?,
        headers,
        text_value(agent, &state.mode)?,
        text_value(agent, &state.credentials)?,
        text_value(agent, &state.redirect)?,
        text_value(agent, &state.referrer_policy)?,
        body,
        Value::FALSE,
    ];
    new_record(agent, object, &values)?;
    Ok(object)
}

fn construct_request(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let input = required_arg(agent, args, 0)?;
    let init = args.get(1).copied().unwrap_or(Value::UNDEFINED);
    let state = request_state(agent, input, init)?;
    new_request(agent, &state)
}

fn request_get<const SLOT: u32>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let record = record(agent, this, REQUEST)?;
    record.get(agent, SLOT)
}

fn request_destination(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    record(agent, this, REQUEST)?;
    text_value(agent, "")
}

/// Members this implementation never varies: `referrer` is always the
/// client (`about:client`), `cache` `default`, `integrity` empty, the
/// navigation flags and `keepalive` false, `duplex` `half` (TD-169).
fn request_constant<const MEMBER: u8>(
    agent: &mut Agent,
    this: Value,
    _: &[Value],
) -> JsResult<Value> {
    record(agent, this, REQUEST)?;
    match MEMBER {
        0 => text_value(agent, "about:client"),
        1 => text_value(agent, "default"),
        2 => text_value(agent, ""),
        3 => Ok(Value::FALSE),
        _ => text_value(agent, "half"),
    }
}

fn request_clone(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let record = record(agent, this, REQUEST)?;
    if record.get(agent, R_USED_REQUEST)? == Value::TRUE {
        return Err(type_error(agent, "the request body is already used"));
    }
    let state = read_request(agent, record)?;
    new_request(agent, &state)
}

// ---- Response ---------------------------------------------------------------

struct ResponseParts<'a> {
    kind: &'a str,
    url: Option<&'a Url>,
    status: u16,
    status_text: &'a [u8],
    headers: &'a HeaderList,
    guard: f64,
    body: Option<&'a [u8]>,
    redirected: bool,
}

fn new_response(agent: &mut Agent, parts: ResponseParts<'_>) -> JsResult<Value> {
    let headers = new_headers(agent, parts.guard, parts.headers)?;
    let proto = prototype(agent, RESPONSE)?;
    let object = Value::object(agent.new_object_with(Some(proto))?);
    let body = match parts.body {
        Some(body) => bytes_value(agent, body)?,
        None => Value::NULL,
    };
    let url = parts
        .url
        .map(Url::serialize_without_fragment)
        .unwrap_or_default();
    let values = [
        text_value(agent, RESPONSE)?,
        text_value(agent, parts.kind)?,
        text_value(agent, &url)?,
        Value::number(f64::from(parts.status)),
        bytes_value(agent, parts.status_text)?,
        headers,
        body,
        Value::FALSE,
        Value::boolean(parts.redirected),
    ];
    new_record(agent, object, &values)?;
    Ok(object)
}

fn construct_response(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let body = args.first().copied().unwrap_or(Value::NULL);
    let init = args.get(1).copied().unwrap_or(Value::UNDEFINED);
    let body = BodyInit::convert(agent, body)?;
    let options = response_options(agent, init)?;
    let body = body.extract(agent)?;
    initialize_response(agent, options, body)
}

struct ResponseOptions {
    status: u16,
    status_text: Vec<u8>,
    headers: HeaderList,
}

fn response_options(agent: &mut Agent, init: Value) -> JsResult<ResponseOptions> {
    let mut options = ResponseOptions {
        status: 200,
        status_text: Vec::new(),
        headers: Vec::new(),
    };
    if init.is_nullish() {
        return Ok(options);
    }
    if init.as_object().is_none() {
        return agent.type_error("Response initialization requires a dictionary");
    }
    with_root(agent, init, |agent| {
        let headers = member(agent, init, "headers")?;
        if !(headers == Value::UNDEFINED) {
            match record_if(agent, headers, HEADERS)? {
                Some(source) => {
                    let (_, source) = read_list(agent, source)?;
                    for (name, value) in source {
                        append(
                            agent,
                            GUARD_RESPONSE,
                            &mut options.headers,
                            name.as_bytes(),
                            &value,
                        )?;
                    }
                }
                None => fill(agent, GUARD_RESPONSE, &mut options.headers, headers)?,
            }
        }
        let value = member(agent, init, "status")?;
        if value != Value::UNDEFINED {
            options.status = to_uint16(agent.to_number(value)?);
        }
        let value = member(agent, init, "statusText")?;
        if value != Value::UNDEFINED {
            options.status_text = to_byte_string(agent, value)?;
        }
        Ok(())
    })?;
    Ok(options)
}

fn initialize_response(
    agent: &mut Agent,
    mut options: ResponseOptions,
    body: Option<ExtractedBody>,
) -> JsResult<Value> {
    if !(200..=599).contains(&options.status) {
        return Err(agent.throw(ErrorKind::RangeError, "status must be 200 to 599"));
    }
    if options
        .status_text
        .iter()
        .any(|&byte| matches!(byte, b'\r' | b'\n'))
    {
        return agent.type_error("invalid statusText");
    }
    if let Some(body) = &body {
        if matches!(options.status, 204 | 205 | 304) {
            return Err(type_error(agent, "a null-body status cannot have a body"));
        }
        append_body_content_type(&mut options.headers, body.content_type.as_deref());
    }
    new_response(
        agent,
        ResponseParts {
            kind: "default",
            url: None,
            status: options.status,
            status_text: &options.status_text,
            headers: &options.headers,
            guard: GUARD_RESPONSE,
            body: body.as_ref().map(|body| body.bytes.as_slice()),
            redirected: false,
        },
    )
}

fn response_json(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let data = required_arg(agent, args, 0)?;
    let options = response_options(agent, args.get(1).copied().unwrap_or(Value::UNDEFINED))?;
    let json = agent.stringify_json(data)?;
    if json == Value::UNDEFINED {
        return agent.type_error("value cannot be serialized to JSON");
    }
    let bytes = to_usv_string(agent, json)?.into_bytes();
    initialize_response(
        agent,
        options,
        Some(ExtractedBody {
            bytes,
            content_type: Some(Cow::Borrowed(b"application/json")),
        }),
    )
}

fn response_error(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    new_response(
        agent,
        ResponseParts {
            kind: "error",
            url: None,
            status: 0,
            status_text: b"",
            headers: &Vec::new(),
            guard: GUARD_IMMUTABLE,
            body: None,
            redirected: false,
        },
    )
}

fn response_get<const SLOT: u32>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let record = record(agent, this, RESPONSE)?;
    record.get(agent, SLOT)
}

fn response_ok(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let record = record(agent, this, RESPONSE)?;
    let status = record.get(agent, 3)?.as_number().unwrap_or(0.0);
    Ok(Value::boolean((200.0..=299.0).contains(&status)))
}

fn response_clone(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let record = record(agent, this, RESPONSE)?;
    if record.get(agent, R_USED_RESPONSE)? == Value::TRUE {
        return Err(type_error(agent, "the response body is already used"));
    }
    let kind = {
        let read = record.get(agent, 1)?;
        value_text(agent, read)
    };
    let url = {
        let read = record.get(agent, 2)?;
        value_text(agent, read)
    };
    let url = (!url.is_empty())
        .then(|| Url::parse(&url, None).ok())
        .flatten();
    let status = record.get(agent, 3)?.as_number().unwrap_or(0.0) as u16;
    let status_text = {
        let read = record.get(agent, 4)?;
        value_bytes(agent, read)
    };
    let headers = record.get(agent, R_HEADERS_RESPONSE)?;
    let headers = self::record(agent, headers, HEADERS)?;
    let (guard, list) = read_list(agent, headers)?;
    let body = record.get(agent, R_BODY_RESPONSE)?;
    let body = (!(body == Value::NULL)).then(|| value_bytes(agent, body));
    let redirected = record.get(agent, 8)? == Value::TRUE;
    new_response(
        agent,
        ResponseParts {
            kind: &kind,
            url: url.as_ref(),
            status,
            status_text: &status_text,
            headers: &list,
            guard,
            body: body.as_deref(),
            redirected,
        },
    )
}

// ---- bodies -----------------------------------------------------------------

fn install_body_methods<const RESPONSE_BODY: bool>(
    agent: &mut Agent,
    proto: ObjectId,
) -> JsResult<()> {
    define_method(
        agent,
        proto,
        "text",
        consume_body::<RESPONSE_BODY, BODY_TEXT>,
        0,
    )?;
    define_method(
        agent,
        proto,
        "json",
        consume_body::<RESPONSE_BODY, BODY_JSON>,
        0,
    )?;
    define_method(
        agent,
        proto,
        "arrayBuffer",
        consume_body::<RESPONSE_BODY, BODY_BUFFER>,
        0,
    )?;
    define_method(
        agent,
        proto,
        "bytes",
        consume_body::<RESPONSE_BODY, BODY_BYTES>,
        0,
    )?;
    define_method(
        agent,
        proto,
        "blob",
        consume_body::<RESPONSE_BODY, BODY_BLOB>,
        0,
    )
}

/// Consume a body once with the conversion selected by the Body method.
fn consume_body<const RESPONSE_BODY: bool, const FORMAT: u8>(
    agent: &mut Agent,
    this: Value,
    _: &[Value],
) -> JsResult<Value> {
    let (interface, body_slot, used_slot) = if RESPONSE_BODY {
        (RESPONSE, R_BODY_RESPONSE, R_USED_RESPONSE)
    } else {
        (REQUEST, R_BODY_REQUEST, R_USED_REQUEST)
    };
    let promise = agent.new_host_promise()?;
    let outcome = (|| -> JsResult<Value> {
        let record = record(agent, this, interface)?;
        if record.get(agent, used_slot)? == Value::TRUE {
            return Err(type_error(agent, "the body is already used"));
        }
        let body = record.get(agent, body_slot)?;
        let bytes = if body == Value::NULL {
            Vec::new()
        } else {
            record.set(agent, used_slot, Value::TRUE)?;
            value_bytes(agent, body)
        };
        let owner = this.as_object().expect("branded Body receiver");
        match FORMAT {
            BODY_BUFFER => return agent.new_web_array_buffer(owner, &bytes),
            BODY_BYTES => return agent.new_web_uint8_array(owner, &bytes),
            BODY_BLOB => {
                let slot = if RESPONSE_BODY {
                    R_HEADERS_RESPONSE
                } else {
                    R_HEADERS_REQUEST
                };
                let headers = record.get(agent, slot)?;
                let content_type = body_mime_type(agent, headers)?;
                return super::blob::from_bytes(agent, bytes, content_type);
            }
            _ => {}
        }
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        let text = String::from_utf8_lossy(bytes);
        if FORMAT == BODY_JSON {
            let units: Vec<u16> = text.encode_utf16().collect();
            agent.parse_json(&units)
        } else {
            text_value(agent, &text)
        }
    })();
    settle(agent, promise, outcome)?;
    Ok(Value::object(promise))
}

fn body_mime_type(agent: &mut Agent, headers: Value) -> JsResult<String> {
    let header_record = record(agent, headers, HEADERS)?;
    let (_, headers) = read_list(agent, header_record)?;
    let mut content_types = rustic_net::Headers::new();
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("content-type") {
            let _ = content_types.append(&name, &value);
        }
    }
    Ok(rustic_net::mime::extract_mime_type(&content_types)
        .map_or_else(String::new, |mime| mime.to_string()))
}

pub(super) fn wasm_response_bytes(agent: &mut Agent, response: Value) -> JsResult<Vec<u8>> {
    let response = record(agent, response, RESPONSE)?;
    let headers = response.get(agent, R_HEADERS_RESPONSE)?;
    let headers = record(agent, headers, HEADERS)?;
    let (_, headers) = read_list(agent, headers)?;
    let mime = combined(&headers, "content-type").unwrap_or_default();
    let first = mime.iter().position(|b| !matches!(b, b' ' | b'\t'));
    let last = mime.iter().rposition(|b| !matches!(b, b' ' | b'\t'));
    if !first
        .zip(last)
        .is_some_and(|(first, last)| mime[first..=last].eq_ignore_ascii_case(b"application/wasm"))
    {
        return agent.type_error("WebAssembly responses require application/wasm");
    }
    let kind = response.get(agent, 1)?;
    if !matches!(
        kind.as_string()
            .map(|id| agent.heap.string(id).to_text())
            .as_deref(),
        Some("basic" | "cors" | "default")
    ) {
        return agent.type_error("WebAssembly response is not readable");
    }
    let status = response.get(agent, 3)?.as_number().unwrap_or_default();
    if !(200.0..=299.0).contains(&status) {
        return agent.type_error("WebAssembly response must have an ok status");
    }
    if response.get(agent, R_USED_RESPONSE)? == Value::TRUE {
        return agent.type_error("the body is already used");
    }
    let body = response.get(agent, R_BODY_RESPONSE)?;
    if body == Value::NULL {
        return Ok(Vec::new());
    }
    response.set(agent, R_USED_RESPONSE, Value::TRUE)?;
    let text = agent
        .heap
        .string(body.as_string().expect("response byte string"));
    if text.len() > super::wasm::MAX_BYTES {
        return Err(super::wasm::compile_error(
            agent,
            "WebAssembly binary exceeds the input limit",
        ));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(text.len())
        .map_err(|_| rustic_js::runtime::OutOfMemory)?;
    bytes.extend(text.units().map(|unit| unit as u8));
    Ok(bytes)
}

pub(super) fn settle(
    agent: &mut Agent,
    promise: ObjectId,
    outcome: JsResult<Value>,
) -> JsResult<()> {
    match outcome {
        Ok(value) => agent.resolve_host_promise(promise, value),
        Err(Abrupt::Throw(reason)) => {
            agent.reject_host_promise(promise, reason);
            Ok(())
        }
        Err(other) => Err(other),
    }
}

// ---- fetch() ----------------------------------------------------------------

fn fetch(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let promise = agent.new_host_promise()?;
    let outcome = (|| -> JsResult<Value> {
        let input = required_arg(agent, args, 0)?;
        let init = args.get(1).copied().unwrap_or(Value::UNDEFINED);
        let state = request_state(agent, input, init)?;
        let host = with_page(agent, |_, page| Ok(page.fetch.borrow().clone()))?;
        let Some(host) = host else {
            return Err(type_error(agent, "Failed to fetch: no network"));
        };
        let request = ScriptRequest {
            method: state.method,
            url: state.url,
            headers: state.headers,
            body: state.body,
            mode: state.mode,
            credentials: state.credentials,
            redirect: state.redirect,
            referrer_policy: state.referrer_policy,
        };
        let Ok(response) = host.fetch(request) else {
            return Err(type_error(agent, "Failed to fetch"));
        };
        new_response(
            agent,
            ResponseParts {
                kind: response.kind,
                url: response.url.as_ref(),
                status: response.status,
                status_text: &response.status_text,
                headers: &response.headers,
                guard: GUARD_IMMUTABLE,
                body: Some(&response.body),
                redirected: response.redirected,
            },
        )
    })();
    settle(agent, promise, outcome)?;
    Ok(Value::object(promise))
}
