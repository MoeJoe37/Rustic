//! Data-backed locale preference queries (ECMA-402 §15.5).

use super::{data, locale, locale_object::locale_text};
use crate::runtime::{Agent, JsResult, Value};

fn lookup<T: Copy>(table: &[(&str, T)], key: &str) -> Option<T> {
    let index = table.binary_search_by_key(&key, |&(name, _)| name).ok()?;
    Some(table[index].1)
}

fn lookup_prefix<T: Copy>(table: &[(&str, T)], mut base: &str) -> Option<T> {
    loop {
        if let Some(value) = lookup(table, base) {
            return Some(value);
        }
        (base, _) = base.rsplit_once('-')?;
    }
}

fn preferred_regions(tag: &str) -> [Option<String>; 2] {
    let [_, _, region, _] = locale::base_fields(locale::split_base(tag).0);
    let region = if !region.is_empty() {
        region.to_owned()
    } else if let Some(region) = locale::subdivision_region(tag, "sd") {
        region
    } else {
        let maximal = locale::adjust_likely_subtags(tag, false);
        let inferred = locale::base_fields(locale::split_base(&maximal).0)[2];
        if inferred.is_empty() { "001" } else { inferred }.to_owned()
    };
    [locale::subdivision_region(tag, "rg"), Some(region)]
}

fn string_array(agent: &mut Agent, strings: &[&str]) -> JsResult<Value> {
    let values = strings
        .iter()
        .map(|string| agent.string(string))
        .collect::<JsResult<Vec<_>>>()?;
    Ok(Value::object(agent.create_array_from_list(&values)?))
}

type Preferences = &'static [(&'static str, &'static [&'static str])];

fn get_preference(
    agent: &mut Agent,
    this: Value,
    key: &str,
    table: Preferences,
    fallback: &[&str],
) -> JsResult<Value> {
    let tag = locale_text(agent, this)?;
    if let Some(value) = locale::unicode_value(&tag, key) {
        return string_array(agent, &[value]);
    }
    let [language, ..] = locale::base_fields(locale::split_base(&tag).0);
    let values = preferred_regions(&tag)
        .into_iter()
        .flatten()
        .find_map(|region| {
            lookup(table, &format!("{language}-{region}")).or_else(|| lookup(table, &region))
        })
        .unwrap_or(fallback);
    string_array(agent, values)
}

pub(super) fn get_calendars(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    get_preference(agent, this, "ca", data::CALENDARS, &["gregory"])
}

pub(super) fn get_hour_cycles(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    get_preference(agent, this, "hc", data::HOUR_CYCLES, &["h23"])
}

pub(super) fn get_collations(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let tag = locale_text(agent, this)?;
    if let Some(value) = locale::unicode_value(&tag, "co") {
        return string_array(agent, &[value]);
    }
    let values = lookup_prefix(data::COLLATIONS, locale::split_base(&tag).0)
        .or_else(|| lookup(data::COLLATIONS, "root"))
        .expect("CLDR root collations");
    string_array(agent, values)
}

pub(super) fn get_numbering_systems(
    agent: &mut Agent,
    this: Value,
    _: &[Value],
) -> JsResult<Value> {
    let tag = locale_text(agent, this)?;
    let system = locale::unicode_value(&tag, "nu")
        .or_else(|| lookup_prefix(data::NUMBERING_SYSTEMS, locale::split_base(&tag).0))
        .unwrap_or("latn");
    string_array(agent, &[system])
}

pub(super) fn get_time_zones(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let tag = locale_text(agent, this)?;
    let [_, _, region, _] = locale::base_fields(locale::split_base(&tag).0);
    if region.is_empty() {
        return Ok(Value::UNDEFINED);
    }
    string_array(agent, lookup(data::TIME_ZONES, region).unwrap_or_default())
}

pub(super) fn get_text_info(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let tag = locale_text(agent, this)?;
    let [_, script, ..] = locale::base_fields(locale::split_base(&tag).0);
    let maximal;
    let script = if script.is_empty() {
        maximal = locale::adjust_likely_subtags(&tag, false);
        locale::base_fields(locale::split_base(&maximal).0)[1]
    } else {
        script
    };
    let direction = match lookup(data::SCRIPT_DIRECTIONS, script) {
        Some(direction) => agent.string(direction)?,
        None => Value::UNDEFINED,
    };
    let object = agent.new_object()?;
    let key = agent.heap.key("direction")?;
    agent.create_data_property_or_throw(object, key, direction)?;
    Ok(Value::object(object))
}

pub(super) fn get_week_info(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let tag = locale_text(agent, this)?;
    let [first_day, start, end] = preferred_regions(&tag)
        .iter()
        .flatten()
        .map(String::as_str)
        .chain(std::iter::once("001"))
        .find_map(|region| lookup(data::WEEK_DATA, region))
        .expect("CLDR world week data");
    let weekdays = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    let first_day = locale::unicode_value(&tag, "fw")
        .and_then(|value| {
            weekdays
                .iter()
                .zip(1..=7)
                .find_map(|(day, number)| (*day == value).then_some(number))
        })
        .unwrap_or(first_day);
    let weekend = (1..=7)
        .filter(|day| {
            if start <= end {
                (start..=end).contains(day)
            } else {
                *day >= start || *day <= end
            }
        })
        .map(|day| Value::number(f64::from(day)))
        .collect::<Vec<_>>();
    let object = agent.new_object()?;
    let key = agent.heap.key("firstDay")?;
    agent.create_data_property_or_throw(object, key, Value::number(f64::from(first_day)))?;
    let key = agent.heap.key("weekend")?;
    let weekend = Value::object(agent.create_array_from_list(&weekend)?);
    agent.create_data_property_or_throw(object, key, weekend)?;
    Ok(Value::object(object))
}
