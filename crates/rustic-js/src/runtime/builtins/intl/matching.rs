//! Locale negotiation shared by ECMA-402 services.

use super::{canonicalize_locale_list, locale};
use crate::runtime::{Agent, ErrorKind, JsResult, ObjectId, Value};

pub(super) fn constructor_options(agent: &mut Agent, value: Value) -> JsResult<Option<ObjectId>> {
    if value == Value::UNDEFINED {
        return Ok(None);
    }
    value
        .as_object()
        .map(Some)
        .ok_or_else(|| agent.throw(ErrorKind::TypeError, "Intl options must be an object"))
}

pub(super) fn option_index(
    agent: &mut Agent,
    options: Option<ObjectId>,
    name: &str,
    choices: &[&str],
) -> JsResult<usize> {
    let Some(options) = options else { return Ok(0) };
    let key = agent.heap.key(name)?;
    let value = agent.get(options, key)?;
    if value == Value::UNDEFINED {
        return Ok(0);
    }
    let text = agent.to_string(value)?;
    agent.heap.charge(agent.heap.string_len(text))?;
    let text = agent.heap.string(text).to_text();
    choices
        .iter()
        .position(|choice| *choice == text)
        .ok_or_else(|| {
            agent.throw(
                ErrorKind::RangeError,
                &format!("invalid Intl {name} option"),
            )
        })
}

fn lookup<T: Copy>(table: &[(&'static str, T)], mut candidate: &str) -> Option<(&'static str, T)> {
    loop {
        if let Ok(index) = table.binary_search_by_key(&candidate, |&(name, _)| name) {
            return Some(table[index]);
        }
        (candidate, _) = candidate.rsplit_once('-')?;
    }
}

fn match_locale<T: Copy>(
    table: &[(&'static str, T)],
    tag: &str,
    best_fit: bool,
) -> Option<(&'static str, T)> {
    let base = locale::split_base(tag).0;
    if let Some(matched) = lookup(table, base) {
        return Some(matched);
    }
    if best_fit {
        let [language, ..] = locale::base_fields(base);
        if language != "und" {
            let maximal = locale::adjust_likely_subtags(base, false);
            return lookup(table, &maximal);
        }
    }
    None
}

pub(super) fn resolve_locale<T: Copy>(
    table: &[(&'static str, T)],
    requested: &[String],
    best_fit: bool,
    default: &'static str,
) -> (&'static str, T) {
    requested
        .iter()
        .find_map(|tag| match_locale(table, tag, best_fit))
        .unwrap_or_else(|| lookup(table, default).expect("default locale is supported"))
}

pub(super) fn supported_locales<T: Copy>(
    agent: &mut Agent,
    locales: Value,
    options: Value,
    table: &[(&'static str, T)],
) -> JsResult<Value> {
    let requested = canonicalize_locale_list(agent, locales)?;
    let options = if options == Value::UNDEFINED {
        None
    } else {
        Some(agent.to_object(options)?)
    };
    let best_fit = option_index(agent, options, "localeMatcher", &["best fit", "lookup"])? == 0;
    let matches = requested
        .iter()
        .filter(|tag| match_locale(table, tag, best_fit).is_some())
        .map(|tag| agent.string(tag))
        .collect::<JsResult<Vec<_>>>()?;
    Ok(Value::object(agent.create_array_from_list(&matches)?))
}
