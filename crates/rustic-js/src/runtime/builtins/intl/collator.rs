//! ECMA-402 locale-sensitive comparison using pinned CLDR collation weights.

mod weights;

use super::super::{arg, install_methods, method, new_constructor, prototype_from_new_target};
use super::{canonicalize_locale_list, data, locale, matching};
use crate::runtime::{
    Agent, ErrorKind, Extra, Internal, JsResult, ObjectId, PropertyDescriptor, PropertyKey,
    StringId, Value,
};

const USAGES: &[&str] = &["sort", "search"];
const CASE_FIRST: &[&str] = &["false", "upper", "lower"];
const SENSITIVITIES: &[&str] = &["variant", "base", "accent", "case"];

#[derive(Clone, Copy, Debug)]
pub(crate) struct Collator {
    pub(crate) locale: StringId,
    pub(crate) bound_compare: Value,
    usage: usize,
    collation: &'static str,
    options: weights::Options,
}

pub(super) fn install(agent: &mut Agent, intl: ObjectId) -> JsResult<()> {
    let prototype = agent.new_object()?;
    agent.intrinsics.collator_prototype = prototype;
    let constructor = new_constructor(agent, "Collator", construct, 0, prototype)?;
    let name = agent.heap.key("Collator")?;
    agent.define_property_or_throw(
        intl,
        name,
        PropertyDescriptor::data_with(Value::object(constructor), true, false, true),
    )?;
    let tag = agent.string("Intl.Collator")?;
    agent.define_property_or_throw(
        prototype,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    let compare = agent.heap.key("compare")?;
    let getter = agent.new_native_function(compare_getter, "get compare", 0)?;
    agent.define_property_or_throw(
        prototype,
        compare,
        PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, false, true),
    )?;
    install_methods(
        agent,
        constructor,
        &[method("supportedLocalesOf", supported_locales, 1)],
    )?;
    install_methods(
        agent,
        prototype,
        &[method("resolvedOptions", resolved_options, 0)],
    )
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let prototype = prototype_from_new_target(agent, agent.intrinsics.collator_prototype)?;
    let object = agent.new_object_with(Some(prototype))?;
    let collator = settings(agent, arg(args, 0), arg(args, 1))?;
    agent
        .heap
        .charge(std::mem::size_of::<Extra>() + std::mem::size_of::<Collator>())?;
    agent.heap.object_mut(object).extra_mut().internal =
        Some(Internal::Collator(Box::new(collator)));
    Ok(Value::object(object))
}

fn supported_locales(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    matching::supported_locales(agent, arg(args, 0), arg(args, 1), data::LIST_LOCALES)
}

fn boolean_option(
    agent: &mut Agent,
    options: Option<ObjectId>,
    name: &str,
) -> JsResult<Option<bool>> {
    let value = matching::get_option(agent, options, name)?;
    Ok((value != Value::UNDEFINED).then(|| agent.to_boolean(value)))
}

fn negotiate<'a>(
    tag: &'a str,
    key: &str,
    option: Option<&str>,
    values: &[&'static str],
) -> (&'static str, Option<&'a str>) {
    let extension = locale::unicode_value(tag, key);
    let extension_value = extension.and_then(|value| {
        let value = if value.is_empty() { "true" } else { value };
        values.iter().copied().find(|choice| *choice == value)
    });
    let option_value =
        option.and_then(|value| values.iter().copied().find(|choice| *choice == value));
    let selected = option_value.or(extension_value).unwrap_or(values[0]);
    (
        selected,
        extension.filter(|_| extension_value == Some(selected)),
    )
}

fn settings(agent: &mut Agent, locales: Value, options: Value) -> JsResult<Collator> {
    let requested = canonicalize_locale_list(agent, locales)?;
    let options = matching::constructor_options(agent, options)?;
    let usage = matching::option_index(agent, options, "usage", USAGES)?;
    let best_fit =
        matching::option_index(agent, options, "localeMatcher", &["best fit", "lookup"])? == 0;
    let collation = matching::string_option(agent, options, "collation")?;
    if collation
        .as_deref()
        .is_some_and(|value| !locale::valid_type(value))
    {
        return Err(agent.throw(ErrorKind::RangeError, "invalid Intl collation option"));
    }
    let numeric = boolean_option(agent, options, "numeric")?;
    let case_first = matching::option_index_optional(agent, options, "caseFirst", CASE_FIRST)?;

    let (base, tag) = requested
        .iter()
        .find_map(|tag| {
            matching::match_locale(data::LIST_LOCALES, tag, best_fit)
                .map(|(base, _)| (base, tag.as_str()))
        })
        .unwrap_or(("en-US", ""));
    let [language, ..] = locale::base_fields(base);
    let collations: &[&str] = if usage == 0 && language == "de" {
        &["default", "phonebk"]
    } else {
        &["default"]
    };
    let (collation, co) = negotiate(
        tag,
        "co",
        collation.as_deref().filter(|value| *value != "default"),
        collations,
    );
    // "default" is the resolved sentinel, never a supported Unicode co value.
    let co = co.filter(|_| collation != "default");
    let numeric_option = numeric.map(|value| if value { "true" } else { "false" });
    let (numeric, kn) = negotiate(tag, "kn", numeric_option, &["false", "true"]);
    let (case_first, kf) = negotiate(
        tag,
        "kf",
        case_first.map(|index| CASE_FIRST[index]),
        CASE_FIRST,
    );
    let sensitivity = matching::option_index(agent, options, "sensitivity", SENSITIVITIES)?;
    let ignore_punctuation =
        boolean_option(agent, options, "ignorePunctuation")?.unwrap_or(language == "th");

    let overrides = [("co", co), ("kn", kn), ("kf", kf)]
        .into_iter()
        .filter_map(|(key, value)| Some((key, value?.to_owned())))
        .collect::<Vec<_>>();
    let resolved = locale::insert_unicode_values(base, &overrides);
    let locale = agent
        .string(&resolved)?
        .as_string()
        .expect("interned locale");
    Ok(Collator {
        locale,
        bound_compare: Value::UNDEFINED,
        usage,
        collation,
        options: weights::Options {
            numeric: numeric == "true",
            ignore_punctuation,
            case_first: CASE_FIRST
                .iter()
                .position(|choice| *choice == case_first)
                .expect("case-first option"),
            sensitivity,
            phonebook: collation == "phonebk" || (usage == 1 && language == "de"),
        },
    })
}

fn branded(agent: &mut Agent, this: Value) -> JsResult<(ObjectId, Collator)> {
    if let Some(object) = this.as_object() {
        if let Some(Internal::Collator(collator)) = agent.heap.object(object).internal() {
            return Ok((object, **collator));
        }
    }
    agent.type_error("expected an Intl.Collator")
}

fn resolved_options(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, collator) = branded(agent, this)?;
    let object = agent.new_object()?;
    let fields = [
        ("locale", Value::string(collator.locale)),
        ("usage", agent.string(USAGES[collator.usage])?),
        (
            "sensitivity",
            agent.string(SENSITIVITIES[collator.options.sensitivity])?,
        ),
        (
            "ignorePunctuation",
            Value::boolean(collator.options.ignore_punctuation),
        ),
        ("collation", agent.string(collator.collation)?),
        ("numeric", Value::boolean(collator.options.numeric)),
        (
            "caseFirst",
            agent.string(CASE_FIRST[collator.options.case_first])?,
        ),
    ];
    for (name, value) in fields {
        let key = agent.heap.key(name)?;
        agent.create_data_property_or_throw(object, key, value)?;
    }
    Ok(Value::object(object))
}

fn compare_getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (object, collator) = branded(agent, this)?;
    if collator.bound_compare != Value::UNDEFINED {
        return Ok(collator.bound_compare);
    }
    let function = agent.new_native_function(bound_compare, "", 2)?;
    agent
        .heap
        .charge(std::mem::size_of::<Extra>() + std::mem::size_of::<Value>())?;
    agent.heap.object_mut(function).extra_mut().internal =
        Some(Internal::Captures(Box::new([this])));
    let result = Value::object(function);
    let Some(Internal::Collator(collator)) = agent.heap.object_mut(object).internal_mut() else {
        unreachable!("brand validated before allocating the callback");
    };
    collator.bound_compare = result;
    Ok(result)
}

fn bound_compare(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let function = agent.active_function().expect("running compare function");
    let Some(Internal::Captures(captures)) = agent.heap.object(function).internal() else {
        unreachable!("compare function captures its collator");
    };
    let (_, collator) = branded(agent, captures[0])?;
    let first = agent.to_string(arg(args, 0))?;
    let second = agent.to_string(arg(args, 1))?;
    compare(agent, first, second, collator.options)
}

pub(in crate::runtime::builtins) fn locale_compare(
    agent: &mut Agent,
    first: StringId,
    second: StringId,
    locales: Value,
    options: Value,
) -> JsResult<Value> {
    let collator = settings(agent, locales, options)?;
    compare(agent, first, second, collator.options)
}

fn compare(
    agent: &mut Agent,
    first: StringId,
    second: StringId,
    options: weights::Options,
) -> JsResult<Value> {
    let order = if first == second {
        std::cmp::Ordering::Equal
    } else {
        let first = weights::key(agent, first, options)?;
        let second = weights::key(agent, second, options)?;
        first.cmp(&second)
    };
    Ok(Value::number(match order {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }))
}
