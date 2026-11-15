//! Locale identifier handling for ECMA-402.

use std::collections::HashSet;

use super::{arg, install_methods, method};
use crate::runtime::{Agent, ErrorKind, JsResult, PropertyDescriptor, PropertyKey, Value};

mod case;
pub(crate) mod collator;
mod data;
mod decimal;
mod list_format;
mod locale;
mod locale_info;
mod locale_object;
mod matching;
mod number_data;
mod number_format;
mod numfmt;
mod pack;
mod plural;
pub(crate) mod plural_rules;
mod service;

pub(super) use number_format::to_locale_string as number_to_locale_string;

/// Indices into `Intrinsics::intl`.
mod slot {
    pub(super) const NUMBER_FORMAT_PROTOTYPE: usize = 0;
    pub(super) const NUMBER_FORMAT: usize = 1;
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let intl = agent.new_object()?;
    install_methods(
        agent,
        intl,
        &[method("getCanonicalLocales", get_canonical_locales, 1)],
    )?;
    locale_object::install(agent, intl)?;
    list_format::install(agent, intl)?;
    collator::install(agent, intl)?;
    plural_rules::install(agent, intl)?;
    number_format::install(agent, intl)?;
    let tag = agent.string("Intl")?;
    let tag_key = PropertyKey::Symbol(agent.symbols.to_string_tag);
    agent.define_property_or_throw(
        intl,
        tag_key,
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    let key = agent.heap.key("Intl")?;
    agent.define_property_or_throw(
        agent.intrinsics.global,
        key,
        PropertyDescriptor::data_with(Value::object(intl), true, false, true),
    )
}

fn get_canonical_locales(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let locales = canonicalize_locale_list(agent, arg(args, 0))?;
    let values = locales
        .iter()
        .map(|tag| agent.string(tag))
        .collect::<JsResult<Vec<_>>>()?;
    Ok(Value::object(agent.create_array_from_list(&values)?))
}

fn locale_input(agent: &mut Agent, tag: Value) -> JsResult<String> {
    if tag.as_string().is_none() && tag.as_object().is_none() {
        return agent.type_error("locale entries must be strings or objects");
    }
    let id = if let Some(id) = locale_object::locale_slot(agent, tag) {
        id
    } else {
        agent.to_string(tag)?
    };
    agent.heap.charge(agent.heap.string_len(id))?;
    Ok(agent.heap.string(id).to_text())
}

fn canonicalize_locale(agent: &mut Agent, tag: Value) -> JsResult<String> {
    let text = locale_input(agent, tag)?;
    locale::canonicalize(&text)
        .ok_or_else(|| agent.throw(ErrorKind::RangeError, "invalid locale identifier"))
}

pub(super) fn canonicalize_locale_list(agent: &mut Agent, locales: Value) -> JsResult<Vec<String>> {
    if locales == Value::UNDEFINED {
        return Ok(Vec::new());
    }
    if locales.as_string().is_some() || locale_object::locale_slot(agent, locales).is_some() {
        return Ok(vec![canonicalize_locale(agent, locales)?]);
    }
    let array = agent.to_object(locales)?;
    let length = agent.length_of_array_like(array)? as u64;
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for index in 0..length {
        agent.check_native_work()?;
        // Charge sparse scans too, even when a direct caller has no step budget.
        agent.heap.charge(1)?;
        let key = match u32::try_from(index) {
            Ok(index) if index != u32::MAX => PropertyKey::Index(index),
            _ => agent.heap.key(&index.to_string())?,
        };
        if agent.has_property(array, key)? {
            let entry = agent.get(array, key)?;
            let tag = canonicalize_locale(agent, entry)?;
            if seen.insert(tag.clone()) {
                agent
                    .heap
                    .charge(2 * (tag.len() + std::mem::size_of::<String>()))?;
                ordered.push(tag);
            }
        }
    }
    Ok(ordered)
}

pub(super) fn locale_case_input(
    agent: &mut Agent,
    units: Vec<u16>,
    locales: Value,
    upper: bool,
) -> JsResult<Vec<u16>> {
    let locales = canonicalize_locale_list(agent, locales)?;
    let language = locales.first().and_then(|tag| tag.split('-').next());
    if let Some(language @ ("az" | "lt" | "tr")) = language {
        agent.heap.charge(
            units
                .len()
                .checked_mul(12)
                .ok_or(crate::runtime::OutOfMemory)?,
        )?;
        Ok(case::tailor(&units, language, upper))
    } else {
        Ok(units)
    }
}
