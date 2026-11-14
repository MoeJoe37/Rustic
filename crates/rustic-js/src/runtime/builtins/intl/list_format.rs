//! ECMA-402 list formatting using pinned CLDR templates.

use super::super::{
    arg, install_methods, iterate, method, new_constructor, prototype_from_new_target,
};
use super::{canonicalize_locale_list, data, matching};
use crate::runtime::{
    Agent, ErrorKind, Internal, JsResult, JsStr, ObjectId, OutOfMemory, PropertyDescriptor,
    PropertyKey, StringId, Value,
};

const TYPES: &[&str] = &["conjunction", "disjunction", "unit"];
const STYLES: &[&str] = &["long", "short", "narrow"];

pub(super) fn install(agent: &mut Agent, intl: ObjectId) -> JsResult<()> {
    let prototype = agent.new_object()?;
    agent.intrinsics.list_format_prototype = prototype;
    let constructor = new_constructor(agent, "ListFormat", construct, 0, prototype)?;
    let key = agent.heap.key("ListFormat")?;
    agent.define_property_or_throw(
        intl,
        key,
        PropertyDescriptor::data_with(Value::object(constructor), true, false, true),
    )?;
    let tag = agent.string("Intl.ListFormat")?;
    agent.define_property_or_throw(
        prototype,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    install_methods(
        agent,
        constructor,
        &[method("supportedLocalesOf", supported_locales, 1)],
    )?;
    install_methods(
        agent,
        prototype,
        &[
            method("format", format, 1),
            method("formatToParts", format_to_parts, 1),
            method("resolvedOptions", resolved_options, 0),
        ],
    )
}

struct Settings {
    locale: &'static str,
    group: usize,
    kind: usize,
    style: usize,
}

fn settings(agent: &mut Agent, this: Value) -> JsResult<Settings> {
    if let Some(object) = this.as_object() {
        if let Some(&Internal::ListFormat {
            locale,
            group,
            kind,
            style,
        }) = agent.heap.object(object).internal()
        {
            return Ok(Settings {
                locale,
                group,
                kind,
                style,
            });
        }
    }
    agent.type_error("expected an Intl.ListFormat")
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if agent.new_target().is_none() {
        return agent.type_error("Intl.ListFormat requires new");
    }
    let prototype = prototype_from_new_target(agent, agent.intrinsics.list_format_prototype)?;
    let requested = canonicalize_locale_list(agent, arg(args, 0))?;
    let options = matching::constructor_options(agent, arg(args, 1))?;
    let best_fit =
        matching::option_index(agent, options, "localeMatcher", &["best fit", "lookup"])? == 0;
    let kind = matching::option_index(agent, options, "type", TYPES)?;
    let style = matching::option_index(agent, options, "style", STYLES)?;
    let (locale, group) =
        matching::resolve_locale(data::LIST_LOCALES, &requested, best_fit, "en-US");
    agent
        .heap
        .charge(std::mem::size_of::<crate::runtime::Extra>())?;
    let object = agent.new_object_with(Some(prototype))?;
    agent.heap.object_mut(object).extra_mut().internal = Some(Internal::ListFormat {
        locale,
        group,
        kind,
        style,
    });
    Ok(Value::object(object))
}

fn supported_locales(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    matching::supported_locales(agent, arg(args, 0), arg(args, 1), data::LIST_LOCALES)
}

fn resolved_options(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let settings = settings(agent, this)?;
    let object = agent.new_object()?;
    for (name, field) in [
        ("locale", settings.locale),
        ("type", TYPES[settings.kind]),
        ("style", STYLES[settings.style]),
    ] {
        let key = agent.heap.key(name)?;
        let value = agent.string(field)?;
        agent.create_data_property_or_throw(object, key, value)?;
    }
    Ok(Value::object(object))
}

fn read_strings(agent: &mut Agent, list: Value) -> JsResult<Vec<StringId>> {
    let mut result = Vec::new();
    if list == Value::UNDEFINED {
        return Ok(result);
    }
    let key = PropertyKey::Symbol(agent.symbols.iterator);
    let Some(iterator) = agent.get_method(list, key)? else {
        return agent.type_error("Intl.ListFormat expects an iterable");
    };
    iterate(agent, list, iterator, |agent, value| {
        let Some(string) = value.as_string() else {
            return Err(agent.throw(
                ErrorKind::TypeError,
                "Intl.ListFormat requires string elements",
            ));
        };
        agent.check_native_work()?;
        agent.heap.charge(std::mem::size_of::<StringId>())?;
        result.push(string);
        Ok(true)
    })?;
    Ok(result)
}

enum Part {
    Element(StringId),
    Literal(&'static str),
}

fn visit_parts(
    strings: &[StringId],
    templates: &[&'static str; 4],
    mut append: impl FnMut(Part) -> JsResult<()>,
) -> JsResult<()> {
    let Some((&last, before_last)) = strings.split_last() else {
        return Ok(());
    };
    let template_at = |index| match (strings.len(), index) {
        (2, _) => templates[0],
        (_, 0) => templates[1],
        (_, index) if index + 1 == before_last.len() => templates[3],
        _ => templates[2],
    };
    // The generator verifies that {0} precedes {1}. Emit the outer
    // prefixes and infixes top-down, then the suffixes in reverse order.
    for (index, &element) in before_last.iter().enumerate() {
        let template = template_at(index);
        let (prefix, head) = template.split_once("{0}").expect("CLDR list template");
        let (infix, _) = head.split_once("{1}").expect("CLDR list template");
        if !prefix.is_empty() {
            append(Part::Literal(prefix))?;
        }
        append(Part::Element(element))?;
        if !infix.is_empty() {
            append(Part::Literal(infix))?;
        }
    }
    append(Part::Element(last))?;
    for index in (0..before_last.len()).rev() {
        let (_, suffix) = template_at(index)
            .split_once("{1}")
            .expect("CLDR list template");
        if !suffix.is_empty() {
            append(Part::Literal(suffix))?;
        }
    }
    Ok(())
}

fn format(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let settings = settings(agent, this)?;
    let strings = read_strings(agent, arg(args, 0))?;
    let templates = &data::LIST_PATTERNS[settings.group][settings.kind * 3 + settings.style];
    let mut units = Vec::new();
    visit_parts(&strings, templates, |part| {
        match part {
            Part::Element(id) => {
                agent.heap.charge(
                    agent
                        .heap
                        .string_len(id)
                        .checked_mul(2)
                        .ok_or(OutOfMemory)?,
                )?;
                units.extend(agent.heap.string(id).units());
            }
            Part::Literal(value) => {
                agent
                    .heap
                    .charge(value.len().checked_mul(2).ok_or(OutOfMemory)?)?;
                units.extend(value.encode_utf16());
            }
        }
        Ok(())
    })?;
    agent.string_from(JsStr::from_units(&units))
}

fn format_to_parts(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let settings = settings(agent, this)?;
    let strings = read_strings(agent, arg(args, 0))?;
    let templates = &data::LIST_PATTERNS[settings.group][settings.kind * 3 + settings.style];
    let type_key = agent.heap.key("type")?;
    let value_key = PropertyKey::String(agent.names.value);
    let mut output = Vec::new();
    visit_parts(&strings, templates, |part| {
        let (kind, value) = match part {
            Part::Element(id) => ("element", Value::string(id)),
            Part::Literal(text) => ("literal", agent.string(text)?),
        };
        let object = agent.new_object()?;
        let kind = agent.string(kind)?;
        agent.create_data_property_or_throw(object, type_key, kind)?;
        agent.create_data_property_or_throw(object, value_key, value)?;
        agent.heap.charge(std::mem::size_of::<Value>())?;
        output.push(Value::object(object));
        Ok(())
    })?;
    Ok(Value::object(agent.create_array_from_list(&output)?))
}
