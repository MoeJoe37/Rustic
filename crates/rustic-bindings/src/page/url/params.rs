//! Ordered form-urlencoded pairs and the live URLSearchParams interface.

use std::collections::HashMap;

use rustic_encoding::Encoding;
use rustic_js::runtime::{
    Agent, JsResult, NativeFn, ObjectId, PropertyDescriptor, PropertyKey, Value,
};
use rustic_url::{UrlLimits, UrlParser, parse_form, serialize_form};

use super::{
    Data, INPUT_LIMIT, PAIR_LIMIT, PARAMS, PARAMS_BYTES_LIMIT, PrivateArray, data, define_method,
    install_interface, limit_error, new_params, object_record, params_record, records, replace,
    required_arg, usv, with_page,
};

type Pairs = Vec<(String, String)>;
const ITERATOR: &str = "URLSearchParams Iterator";
const ENTRIES: u8 = 0;
const KEYS: u8 = 1;
const VALUES: u8 = 2;

pub(super) fn install(agent: &mut Agent) -> JsResult<ObjectId> {
    install_interface(
        agent,
        PARAMS,
        None,
        construct,
        0,
        &[("size", "get size", size)],
        |agent, proto| {
            for (name, method, length) in [
                ("append", append as NativeFn, 2),
                ("delete", delete, 1),
                ("get", get, 1),
                ("getAll", get_all, 1),
                ("has", has, 1),
                ("set", set, 2),
                ("sort", sort, 0),
                ("toString", to_string, 0),
                ("forEach", for_each, 1),
                ("entries", iterator::<ENTRIES>, 0),
                ("keys", iterator::<KEYS>, 0),
                ("values", iterator::<VALUES>, 0),
            ] {
                define_method(agent, proto, name, method, length)?;
            }
            let key = agent.heap.key("entries")?;
            let entries = agent.get(proto, key)?;
            agent.define_property_or_throw(
                proto,
                PropertyKey::Symbol(agent.symbols.iterator),
                PropertyDescriptor::data_with(entries, true, false, true),
            )
        },
    )?;

    let parent = agent.get_prototype_of(agent.intrinsics.array_iterator_prototype)?;
    let iterator = agent.new_object_with(parent)?;
    define_method(agent, iterator, "next", iterator_next, 0)?;
    let tag = Value::string(agent.heap.intern_str(ITERATOR)?);
    agent.define_property_or_throw(
        iterator,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    Ok(iterator)
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("URLSearchParams requires construction");
    };
    let input = args.first().copied().unwrap_or(Value::UNDEFINED);
    let pairs = match input.as_object() {
        Some(object) => {
            let key = PropertyKey::Symbol(agent.symbols.iterator);
            match agent.get_method(input, key)? {
                Some(method) => from_sequence(agent, input, method)?,
                None => from_record(agent, object)?,
            }
        }
        None if input == Value::UNDEFINED => Vec::new(),
        None => {
            let text = usv(agent, input, INPUT_LIMIT)?;
            parse_form(text.strip_prefix('?').unwrap_or(&text).as_bytes())
        }
    };
    let prototype = agent.host_prototype_from_new_target(PARAMS, target)?;
    new_params(
        agent,
        prototype,
        Value::UNDEFINED,
        Data { url: None, pairs },
    )
}

fn from_sequence(agent: &mut Agent, input: Value, method: Value) -> JsResult<Pairs> {
    let mut result = Vec::new();
    let mut bytes = 0;
    let mut invalid_size = false;
    agent.iterate_values_with_method(input, method, |agent, item| {
        let mut name = String::new();
        let mut value = String::new();
        let mut length = 0_usize;
        agent.iterate_values(item, |agent, part| {
            let text = usv(agent, part, INPUT_LIMIT)?;
            match length {
                0 => name = text,
                1 => value = text,
                _ => {}
            }
            length += 1;
            Ok(true)
        })?;
        invalid_size |= length != 2;
        bytes += name.len() + value.len();
        if bytes > PARAMS_BYTES_LIMIT || result.len() >= PAIR_LIMIT {
            return limit_error(agent);
        }
        result.push((name, value));
        Ok(true)
    })?;
    if invalid_size {
        return agent.type_error("URLSearchParams requires pairs of two items");
    }
    Ok(result)
}

fn from_record(agent: &mut Agent, object: ObjectId) -> JsResult<Pairs> {
    let mut result: Pairs = Vec::new();
    let mut positions: HashMap<String, usize> = HashMap::new();
    let mut bytes = 0;
    for key in agent.own_property_keys(object)? {
        agent.check_native_work()?;
        if matches!(key, PropertyKey::Symbol(_))
            || !agent
                .get_own_property(object, key)?
                .is_some_and(|descriptor| descriptor.enumerable == Some(true))
        {
            continue;
        }
        let name = match key {
            PropertyKey::Index(index) => index.to_string(),
            PropertyKey::String(name) => usv(agent, Value::string(name), INPUT_LIMIT)?,
            PropertyKey::Symbol(_) => unreachable!("symbol keys were skipped"),
        };
        let value = agent.get(object, key)?;
        let value = usv(agent, value, INPUT_LIMIT)?;
        bytes += value.len();
        if let Some(&index) = positions.get(&name) {
            bytes -= result[index].1.len();
            result[index].1 = value;
        } else {
            bytes += name.len();
            positions.insert(name.clone(), result.len());
            result.push((name, value));
        }
        if bytes > PARAMS_BYTES_LIMIT || result.len() > PAIR_LIMIT {
            return limit_error(agent);
        }
    }
    Ok(result)
}

fn size(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let record = params_record(agent, this)?;
    Ok(Value::number(data(agent, record)?.pairs.len() as f64))
}

fn name_argument(agent: &mut Agent, args: &[Value]) -> JsResult<String> {
    let name = required_arg(agent, args, 0)?;
    usv(agent, name, INPUT_LIMIT)
}

fn filter_argument(agent: &mut Agent, args: &[Value]) -> JsResult<Option<String>> {
    let value = args.get(1).copied().unwrap_or(Value::UNDEFINED);
    if value == Value::UNDEFINED {
        Ok(None)
    } else {
        usv(agent, value, INPUT_LIMIT).map(Some)
    }
}

fn get(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = params_record(agent, this)?;
    let name = name_argument(agent, args)?;
    let current = data(agent, record)?;
    match current.pairs.iter().find(|(key, _)| *key == name) {
        Some((_, value)) => agent.string(value),
        None => Ok(Value::NULL),
    }
}

fn get_all(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = params_record(agent, this)?;
    let name = name_argument(agent, args)?;
    let current = data(agent, record)?;
    let result = agent.new_array()?;
    let mut index = 0;
    for (key, value) in &current.pairs {
        if *key == name {
            let value = agent.string(value)?;
            agent.create_data_property_or_throw(result, PropertyKey::Index(index), value)?;
            index += 1;
        }
    }
    Ok(Value::object(result))
}

fn has(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = params_record(agent, this)?;
    let name = name_argument(agent, args)?;
    let filter = filter_argument(agent, args)?;
    let current = data(agent, record)?;
    Ok(Value::boolean(current.pairs.iter().any(|(key, value)| {
        *key == name && filter.as_ref().is_none_or(|filter| value == filter)
    })))
}

fn append(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = params_record(agent, this)?;
    let value = required_arg(agent, args, 1)?;
    let name = name_argument(agent, args)?;
    let value = usv(agent, value, INPUT_LIMIT)?;
    change(agent, record, |pairs| pairs.push((name, value)))
}

fn delete(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = params_record(agent, this)?;
    let name = name_argument(agent, args)?;
    let filter = filter_argument(agent, args)?;
    change(agent, record, |pairs| {
        pairs.retain(|(key, value)| {
            *key != name || filter.as_ref().is_some_and(|filter| value != filter)
        });
    })
}

fn set(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = params_record(agent, this)?;
    let value = required_arg(agent, args, 1)?;
    let name = name_argument(agent, args)?;
    let mut value = Some(usv(agent, value, INPUT_LIMIT)?);
    change(agent, record, |pairs| {
        pairs.retain_mut(|(key, current)| {
            if *key != name {
                return true;
            }
            match value.take() {
                Some(value) => {
                    *current = value;
                    true
                }
                None => false,
            }
        });
        if let Some(value) = value {
            pairs.push((name, value));
        }
    })
}

fn sort(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let record = params_record(agent, this)?;
    change(agent, record, |pairs| {
        pairs.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()))
    })
}

fn change(
    agent: &mut Agent,
    record: PrivateArray,
    update: impl FnOnce(&mut Pairs),
) -> JsResult<Value> {
    let current = data(agent, record)?;
    agent.with_host_memory(current.heap_bytes(), |agent| {
        let mut next = (*current).clone();
        update(&mut next.pairs);
        let bytes = next
            .pairs
            .iter()
            .map(|(name, value)| name.len() + value.len())
            .sum::<usize>();
        if bytes > PARAMS_BYTES_LIMIT || next.pairs.len() > PAIR_LIMIT {
            return limit_error(agent);
        }
        if let Some(url) = &next.url {
            let query = serialize_form(&next.pairs, Encoding::Utf8);
            let limits = UrlLimits {
                max_input_bytes: PARAMS_BYTES_LIMIT,
                ..UrlLimits::default()
            };
            let parser = UrlParser::new(limits, rustic_base::CancellationToken::default());
            next.url = Some(
                parser
                    .with_component(url, rustic_url::UrlComponent::Search, &query)
                    .map_err(|_| {
                        agent.throw(
                            rustic_js::runtime::ErrorKind::RangeError,
                            "URL resource limit exceeded",
                        )
                    })?,
            );
        }
        replace(agent, record, next)?;
        Ok(Value::UNDEFINED)
    })
}

fn to_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let record = params_record(agent, this)?;
    let current = data(agent, record)?;
    agent.string(&serialize_form(&current.pairs, Encoding::Utf8))
}

fn for_each(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = params_record(agent, this)?;
    let callback = required_arg(agent, args, 0)?;
    if !agent.is_callable(callback) {
        return agent.type_error("URLSearchParams callback is not callable");
    }
    let receiver = args.get(1).copied().unwrap_or(Value::UNDEFINED);
    for index in 0.. {
        agent.check_native_work()?;
        let current = data(agent, record)?;
        let Some((name, value)) = current.pairs.get(index) else {
            break;
        };
        let name = agent.string(name)?;
        let value = agent.string(value)?;
        drop(current);
        agent.call(callback, receiver, &[value, name, this])?;
    }
    Ok(Value::UNDEFINED)
}

fn iterator<const KIND: u8>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    params_record(agent, this)?;
    let prototype = with_page(agent, |_, page| Ok(page.urls.iterator))?;
    let object = Value::object(agent.new_host_state(ITERATOR, prototype, (), 0)?);
    let record = PrivateArray::new(
        agent,
        &[this, Value::number(f64::from(KIND)), Value::number(0.0)],
    )?;
    let records = records(agent)?;
    agent.weak_map_set(records, object, record.value())?;
    Ok(object)
}

fn iterator_next(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let iterator = PrivateArray::from_value(object_record(agent, this, ITERATOR)?)
        .expect("private iterator record");
    let target = iterator.get(agent, 0)?;
    let kind = iterator.get(agent, 1)?.as_number().expect("iterator kind") as u8;
    let index = iterator.get(agent, 2)?.as_number().expect("iterator index") as usize;
    let record = params_record(agent, target)?;
    let current = data(agent, record)?;
    let (value, done) = match current.pairs.get(index) {
        Some((name, value)) => {
            iterator.set(agent, 2, Value::number((index + 1) as f64))?;
            let value = match kind {
                KEYS => agent.string(name)?,
                VALUES => agent.string(value)?,
                _ => {
                    let name = agent.string(name)?;
                    let value = agent.string(value)?;
                    Value::object(agent.create_array_from_list(&[name, value])?)
                }
            };
            (value, false)
        }
        None => (Value::UNDEFINED, true),
    };
    let result = agent.new_object()?;
    agent.create_data_property_or_throw(result, PropertyKey::String(agent.names.value), value)?;
    agent.create_data_property_or_throw(
        result,
        PropertyKey::String(agent.names.done),
        Value::boolean(done),
    )?;
    Ok(Value::object(result))
}
