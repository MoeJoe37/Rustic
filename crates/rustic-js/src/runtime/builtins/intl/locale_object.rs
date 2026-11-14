//! Intl.Locale construction and identifier accessors (ECMA-402 §15).

use super::super::{arg, install_methods, method, new_constructor, prototype_from_new_target};
use super::{locale, locale_info, locale_input};
use crate::runtime::{
    Agent, ErrorKind, Internal, JsResult, NativeFn, ObjectId, PropertyDescriptor, PropertyKey,
    StringId, Value,
};

const OPTIONS: [(&str, &str); 7] = [
    ("calendar", "ca"),
    ("collation", "co"),
    ("firstDayOfWeek", "fw"),
    ("hourCycle", "hc"),
    ("caseFirst", "kf"),
    ("numeric", "kn"),
    ("numberingSystem", "nu"),
];

pub(super) fn install(agent: &mut Agent, intl: ObjectId) -> JsResult<()> {
    let prototype = agent.new_object()?;
    agent.intrinsics.locale_prototype = prototype;
    let constructor = new_constructor(agent, "Locale", construct, 1, prototype)?;
    let key = agent.heap.key("Locale")?;
    agent.define_property_or_throw(
        intl,
        key,
        PropertyDescriptor::data_with(Value::object(constructor), true, false, true),
    )?;
    let tag = agent.string("Intl.Locale")?;
    agent.define_property_or_throw(
        prototype,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    install_methods(
        agent,
        prototype,
        &[
            method("toString", to_string, 0),
            method("maximize", maximize, 0),
            method("minimize", minimize, 0),
            method("getCalendars", locale_info::get_calendars, 0),
            method("getCollations", locale_info::get_collations, 0),
            method("getHourCycles", locale_info::get_hour_cycles, 0),
            method("getNumberingSystems", locale_info::get_numbering_systems, 0),
            method("getTimeZones", locale_info::get_time_zones, 0),
            method("getTextInfo", locale_info::get_text_info, 0),
            method("getWeekInfo", locale_info::get_week_info, 0),
        ],
    )?;
    let accessors: &[(&str, NativeFn)] = &[
        ("baseName", base_name),
        ("language", |agent, this, _| field(agent, this, 0)),
        ("script", |agent, this, _| field(agent, this, 1)),
        ("region", |agent, this, _| field(agent, this, 2)),
        ("variants", |agent, this, _| field(agent, this, 3)),
        ("calendar", |agent, this, _| extension(agent, this, "ca")),
        ("collation", |agent, this, _| extension(agent, this, "co")),
        ("firstDayOfWeek", |agent, this, _| {
            extension(agent, this, "fw")
        }),
        ("hourCycle", |agent, this, _| extension(agent, this, "hc")),
        ("caseFirst", |agent, this, _| extension(agent, this, "kf")),
        ("numeric", numeric),
        ("numberingSystem", |agent, this, _| {
            extension(agent, this, "nu")
        }),
    ];
    for &(name, callback) in accessors {
        let getter = agent.new_native_function(callback, &format!("get {name}"), 0)?;
        let key = agent.heap.key(name)?;
        agent.define_property_or_throw(
            prototype,
            key,
            PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, false, true),
        )?;
    }
    Ok(())
}

pub(super) fn locale_slot(agent: &Agent, value: Value) -> Option<StringId> {
    match agent.heap.object(value.as_object()?).internal() {
        Some(Internal::Locale(tag)) => Some(*tag),
        _ => None,
    }
}

fn require_locale(agent: &mut Agent, value: Value) -> JsResult<StringId> {
    locale_slot(agent, value)
        .ok_or_else(|| agent.throw(ErrorKind::TypeError, "expected an Intl.Locale"))
}

pub(super) fn locale_text(agent: &mut Agent, value: Value) -> JsResult<String> {
    let tag = require_locale(agent, value)?;
    agent.heap.charge(agent.heap.string_len(tag))?;
    Ok(agent.heap.string(tag).to_text())
}

fn new_locale(agent: &mut Agent, prototype: ObjectId, tag: &str) -> JsResult<Value> {
    let tag = agent.heap.alloc_str(tag)?;
    agent
        .heap
        .charge(std::mem::size_of::<crate::runtime::Extra>())?;
    let object = agent.new_object_with(Some(prototype))?;
    agent.heap.object_mut(object).extra_mut().internal = Some(Internal::Locale(tag));
    Ok(Value::object(object))
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if agent.new_target().is_none() {
        return agent.type_error("Intl.Locale requires new");
    }
    let prototype = prototype_from_new_target(agent, agent.intrinsics.locale_prototype)?;
    let input = locale_input(agent, arg(args, 0))?;
    let options = arg(args, 1);
    let options = if options == Value::UNDEFINED {
        None
    } else {
        Some(agent.to_object(options)?)
    };
    let tag = canonical_tag(agent, &input)?;
    let Some(options) = options else {
        return new_locale(agent, prototype, &tag);
    };

    let (base, suffix) = locale::split_base(&tag);
    let mut fields = locale::base_fields(base).map(str::to_owned);
    for (index, name) in ["language", "script", "region", "variants"]
        .into_iter()
        .enumerate()
    {
        let value = get_option(agent, options, name)?;
        if value != Value::UNDEFINED {
            let value = option_text(agent, value)?;
            if !locale::valid_option(index, &value) {
                return invalid_option(agent, name);
            }
            fields[index] = value;
        }
    }
    let mut tag = fields
        .iter()
        .filter(|field| !field.is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("-");
    tag.push_str(suffix);

    let mut overrides = Vec::new();
    for (name, key) in OPTIONS {
        let value = get_option(agent, options, name)?;
        if value == Value::UNDEFINED {
            continue;
        }
        let text = if key == "kn" {
            agent.to_boolean(value).to_string()
        } else {
            option_text(agent, value)?
        };
        let text = if key == "fw" {
            weekday_option(&text)
        } else {
            &text
        };
        let valid = match key {
            "hc" => matches!(text, "h11" | "h12" | "h23" | "h24"),
            "kf" => matches!(text, "upper" | "lower" | "false"),
            _ => locale::valid_type(text),
        };
        if !valid {
            return invalid_option(agent, name);
        }
        overrides.push((key, text.to_ascii_lowercase()));
    }
    let tag = canonical_tag(agent, &tag)?;
    let tag = canonical_tag(agent, &locale::insert_unicode_values(&tag, &overrides))?;
    new_locale(agent, prototype, &tag)
}

fn weekday_option(value: &str) -> &str {
    match value {
        "0" | "7" => "sun",
        "1" => "mon",
        "2" => "tue",
        "3" => "wed",
        "4" => "thu",
        "5" => "fri",
        "6" => "sat",
        _ => value,
    }
}

fn get_option(agent: &mut Agent, options: ObjectId, name: &str) -> JsResult<Value> {
    let key = agent.heap.key(name)?;
    agent.get(options, key)
}

fn option_text(agent: &mut Agent, value: Value) -> JsResult<String> {
    let text = agent.to_string(value)?;
    agent.heap.charge(agent.heap.string_len(text))?;
    Ok(agent.heap.string(text).to_text())
}

fn invalid_option<T>(agent: &mut Agent, name: &str) -> JsResult<T> {
    Err(agent.throw(
        ErrorKind::RangeError,
        &format!("invalid Intl.Locale {name} option"),
    ))
}

fn canonical_tag(agent: &mut Agent, tag: &str) -> JsResult<String> {
    locale::canonicalize(tag)
        .ok_or_else(|| agent.throw(ErrorKind::RangeError, "invalid locale identifier"))
}

fn to_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::string(require_locale(agent, this)?))
}

fn adjusted(agent: &mut Agent, this: Value, minimize: bool) -> JsResult<Value> {
    let tag = locale_text(agent, this)?;
    let result = locale::adjust_likely_subtags(&tag, minimize);
    new_locale(agent, agent.intrinsics.locale_prototype, &result)
}

fn maximize(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    adjusted(agent, this, false)
}

fn minimize(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    adjusted(agent, this, true)
}

fn base_name(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let tag = locale_text(agent, this)?;
    agent.string(locale::split_base(&tag).0)
}

fn field(agent: &mut Agent, this: Value, index: usize) -> JsResult<Value> {
    let tag = locale_text(agent, this)?;
    let part = locale::base_fields(locale::split_base(&tag).0)[index];
    if part.is_empty() {
        Ok(Value::UNDEFINED)
    } else {
        agent.string(part)
    }
}

fn extension(agent: &mut Agent, this: Value, key: &str) -> JsResult<Value> {
    let tag = locale_text(agent, this)?;
    match locale::unicode_value(&tag, key) {
        Some(value) => agent.string(value),
        None => Ok(Value::UNDEFINED),
    }
}

fn numeric(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let tag = locale_text(agent, this)?;
    Ok(Value::boolean(
        locale::unicode_value(&tag, "kn") == Some(""),
    ))
}
