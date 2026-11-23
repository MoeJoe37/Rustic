//! Window URL bindings. A private, traced record connects URL, its stable
//! URLSearchParams object, and their current immutable Rust data.

use std::rc::Rc;

use rustic_js::runtime::{
    Agent, ErrorKind, JsResult, NativeFn, ObjectId, PropertyDescriptor, Value,
};
use rustic_url::{ParseError, Url, UrlComponent, UrlParser, parse_form};

use super::{
    events::{PrivateArray, define_method},
    install_interface, prototype, required_arg, with_page,
};
use crate::webidl::coerce_string;

mod params;

const URL: &str = "URL";
const PARAMS: &str = "URLSearchParams";
const DATA: &str = "internal URL data";
const DATA_SLOT: u32 = 0;

const INPUT_LIMIT: usize = 64 * 1024;
const PARAMS_BYTES_LIMIT: usize = 256 * 1024;
const PAIR_LIMIT: usize = INPUT_LIMIT;

const HREF: usize = 0;
const ORIGIN: usize = 1;
const PROTOCOL: usize = 2;
const USERNAME: usize = 3;
const PASSWORD: usize = 4;
const HOST: usize = 5;
const HOSTNAME: usize = 6;
const PORT: usize = 7;
const PATHNAME: usize = 8;
const SEARCH: usize = 9;
const HASH: usize = 10;

const COMPONENTS: [UrlComponent; 9] = [
    UrlComponent::Protocol,
    UrlComponent::Username,
    UrlComponent::Password,
    UrlComponent::Host,
    UrlComponent::Hostname,
    UrlComponent::Port,
    UrlComponent::Pathname,
    UrlComponent::Search,
    UrlComponent::Hash,
];

#[derive(Debug)]
pub(super) struct Objects {
    records: ObjectId,
    iterator: ObjectId,
}

#[derive(Clone)]
struct Data {
    url: Option<Url>,
    pairs: Vec<(String, String)>,
}

impl Data {
    fn parsed(url: Url) -> Self {
        let pairs = url
            .query()
            .map_or_else(Vec::new, |query| parse_form(query.as_bytes()));
        Self {
            url: Some(url),
            pairs,
        }
    }

    fn heap_bytes(&self) -> usize {
        self.url.as_ref().map_or(0, Url::heap_bytes)
            + self.pairs.capacity() * size_of::<(String, String)>()
            + self
                .pairs
                .iter()
                .map(|(name, value)| name.capacity() + value.capacity())
                .sum::<usize>()
    }
}

pub(super) fn install(agent: &mut Agent) -> JsResult<Objects> {
    install_interface(agent, URL, None, construct_url, 1, &[], |agent, proto| {
        for (name, getter_name, getter, setter_name, setter) in [
            (
                "href",
                "get href",
                url_get::<HREF> as NativeFn,
                "set href",
                Some(url_set::<HREF> as NativeFn),
            ),
            ("origin", "get origin", url_get::<ORIGIN>, "", None),
            (
                "protocol",
                "get protocol",
                url_get::<PROTOCOL>,
                "set protocol",
                Some(url_set::<PROTOCOL>),
            ),
            (
                "username",
                "get username",
                url_get::<USERNAME>,
                "set username",
                Some(url_set::<USERNAME>),
            ),
            (
                "password",
                "get password",
                url_get::<PASSWORD>,
                "set password",
                Some(url_set::<PASSWORD>),
            ),
            (
                "host",
                "get host",
                url_get::<HOST>,
                "set host",
                Some(url_set::<HOST>),
            ),
            (
                "hostname",
                "get hostname",
                url_get::<HOSTNAME>,
                "set hostname",
                Some(url_set::<HOSTNAME>),
            ),
            (
                "port",
                "get port",
                url_get::<PORT>,
                "set port",
                Some(url_set::<PORT>),
            ),
            (
                "pathname",
                "get pathname",
                url_get::<PATHNAME>,
                "set pathname",
                Some(url_set::<PATHNAME>),
            ),
            (
                "search",
                "get search",
                url_get::<SEARCH>,
                "set search",
                Some(url_set::<SEARCH>),
            ),
            (
                "hash",
                "get hash",
                url_get::<HASH>,
                "set hash",
                Some(url_set::<HASH>),
            ),
            (
                "searchParams",
                "get searchParams",
                url_search_params,
                "",
                None,
            ),
        ] {
            let getter = Value::object(agent.new_native_function(getter, getter_name, 0)?);
            let setter = match setter {
                Some(setter) => Value::object(agent.new_native_function(setter, setter_name, 1)?),
                None => Value::UNDEFINED,
            };
            let key = agent.heap.key(name)?;
            agent.define_property_or_throw(
                proto,
                key,
                PropertyDescriptor::accessor(getter, setter, true, true),
            )?;
        }
        define_method(agent, proto, "toString", url_get::<HREF>, 0)?;
        define_method(agent, proto, "toJSON", url_get::<HREF>, 0)
    })?;
    let url = prototype(agent, URL)?;
    let constructor = agent
        .get(
            url,
            rustic_js::runtime::PropertyKey::String(agent.names.constructor),
        )?
        .as_object()
        .expect("installed URL constructor");
    define_method(agent, constructor, "canParse", can_parse, 1)?;
    define_method(agent, constructor, "parse", parse, 1)?;

    let iterator = params::install(agent)?;
    agent.root(Value::object(iterator))?;
    let records = agent.new_weak_map()?;
    agent.root(Value::object(records))?;
    Ok(Objects { records, iterator })
}

fn construct_url(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("URL requires construction");
    };
    let Some(url) = parse_arguments(agent, args)? else {
        return agent.type_error("invalid URL");
    };
    let prototype = agent.host_prototype_from_new_target(URL, target)?;
    new_url(agent, prototype, url)
}

fn can_parse(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(parse_arguments(agent, args)?.is_some()))
}

fn parse(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(url) = parse_arguments(agent, args)? else {
        return Ok(Value::NULL);
    };
    let prototype = prototype(agent, URL)?;
    new_url(agent, prototype, url)
}

fn parse_arguments(agent: &mut Agent, args: &[Value]) -> JsResult<Option<Url>> {
    let input = required_arg(agent, args, 0)?;
    let input = usv(agent, input, INPUT_LIMIT)?;
    let base = args.get(1).copied().unwrap_or(Value::UNDEFINED);
    let parser = UrlParser::default();
    let base = if base == Value::UNDEFINED {
        None
    } else {
        let base = usv(agent, base, INPUT_LIMIT)?;
        let Ok(base) = parser.parse(&base, None) else {
            return Ok(None);
        };
        Some(base)
    };
    Ok(parser.parse(&input, base.as_ref()).ok())
}

fn new_url(agent: &mut Agent, prototype: ObjectId, url: Url) -> JsResult<Value> {
    let object = Value::object(agent.new_host_state(URL, prototype, (), 0)?);
    let params_proto = super::prototype(agent, PARAMS)?;
    let params = new_params(agent, params_proto, object, Data::parsed(url))?;
    let records = records(agent)?;
    agent.weak_map_set(records, object, params)?;
    Ok(object)
}

fn new_params(agent: &mut Agent, prototype: ObjectId, owner: Value, data: Data) -> JsResult<Value> {
    let object = Value::object(agent.new_host_state(PARAMS, prototype, (), 0)?);
    let data = store_data(agent, data)?;
    let record = PrivateArray::new(agent, &[data, owner])?;
    let records = records(agent)?;
    agent.weak_map_set(records, object, record.value())?;
    Ok(object)
}

fn store_data(agent: &mut Agent, data: Data) -> JsResult<Value> {
    let bytes = data.heap_bytes();
    let prototype = agent.intrinsics.object_prototype;
    Ok(Value::object(
        agent.new_host_state(DATA, prototype, data, bytes)?,
    ))
}

fn records(agent: &mut Agent) -> JsResult<ObjectId> {
    with_page(agent, |_, page| Ok(page.urls.records))
}

fn object_record(agent: &mut Agent, this: Value, brand: &'static str) -> JsResult<Value> {
    if this
        .as_object()
        .is_some_and(|object| agent.host_state::<()>(object, brand).is_some())
    {
        let records = records(agent)?;
        let record = agent.weak_map_get(records, this);
        if record.as_object().is_some() {
            return Ok(record);
        }
    }
    agent.type_error(&format!("invalid {brand} receiver"))
}

fn params_record(agent: &mut Agent, this: Value) -> JsResult<PrivateArray> {
    Ok(
        PrivateArray::from_value(object_record(agent, this, PARAMS)?)
            .expect("private params record"),
    )
}

fn data(agent: &mut Agent, record: PrivateArray) -> JsResult<Rc<Data>> {
    let holder = record
        .get(agent, DATA_SLOT)?
        .as_object()
        .expect("private URL data");
    Ok(agent
        .host_state(holder, DATA)
        .expect("private URL data brand"))
}

fn replace(agent: &mut Agent, record: PrivateArray, next: Data) -> JsResult<()> {
    let value = store_data(agent, next)?;
    record.set(agent, DATA_SLOT, value)
}

fn url_search_params(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    object_record(agent, this, URL)
}

fn url_get<const GET: usize>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let params = url_search_params(agent, this, &[])?;
    let record = params_record(agent, params)?;
    let data = data(agent, record)?;
    let url = data.url.as_ref().expect("URL record");
    let value = match GET {
        HREF => url.serialize(),
        ORIGIN => url.origin().serialize(),
        PROTOCOL => format!("{}:", url.scheme()),
        USERNAME => return agent.string(url.username()),
        PASSWORD => return agent.string(url.password()),
        HOST => match url.port() {
            Some(port) => format!(
                "{}:{port}",
                url.host()
                    .map_or_else(String::new, rustic_url::Host::serialize)
            ),
            None => url
                .host()
                .map_or_else(String::new, rustic_url::Host::serialize),
        },
        HOSTNAME => url
            .host()
            .map_or_else(String::new, rustic_url::Host::serialize),
        PORT => url.port().map_or_else(String::new, |port| port.to_string()),
        PATHNAME => url.pathname(),
        SEARCH => prefixed('?', url.query()),
        HASH => prefixed('#', url.fragment()),
        _ => unreachable!("URL getter"),
    };
    agent.string(&value)
}

fn prefixed(prefix: char, value: Option<&str>) -> String {
    value
        .filter(|value| !value.is_empty())
        .map_or_else(String::new, |value| format!("{prefix}{value}"))
}

fn url_set<const SET: usize>(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let params = url_search_params(agent, this, &[])?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let value = usv(agent, value, INPUT_LIMIT)?;
    let record = params_record(agent, params)?;
    let parser = UrlParser::default();
    if SET == HREF {
        let Ok(url) = parser.parse(&value, None) else {
            return agent.type_error("invalid URL");
        };
        replace(agent, record, Data::parsed(url))?;
    } else {
        let current = data(agent, record)?;
        let base = current.url.as_ref().expect("URL record");
        let url = match parser.with_component(base, COMPONENTS[SET - PROTOCOL], &value) {
            Ok(url) => url,
            Err(ParseError::UnsupportedBlob) => return Ok(Value::UNDEFINED),
            Err(_) => return limit_error(agent),
        };
        if SET == SEARCH {
            replace(agent, record, Data::parsed(url))?;
        } else if url != *base {
            let next = Data {
                url: Some(url),
                pairs: current.pairs.clone(),
            };
            replace(agent, record, next)?;
        }
    }
    Ok(Value::UNDEFINED)
}

fn usv(agent: &mut Agent, value: Value, limit: usize) -> JsResult<String> {
    let id = coerce_string(agent, value)?;
    if agent.heap.string_len(id) > limit {
        return limit_error(agent);
    }
    agent.heap.flatten(id)?;
    let text = agent.heap.string(id).to_text();
    if text.len() > limit {
        return limit_error(agent);
    }
    Ok(text)
}

fn limit_error<T>(agent: &mut Agent) -> JsResult<T> {
    Err(agent.throw(ErrorKind::RangeError, "URL resource limit exceeded"))
}
