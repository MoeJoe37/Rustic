//! A read-only CSS Object Model ([CSSOM §6]): `document.styleSheets`,
//! `StyleSheetList`, `CSSStyleSheet`, `CSSRuleList`, `MediaList` and the
//! rule interfaces, including `CSSScopeRule` ([CSS Cascade 6 §3.5.3]).
//!
//! A `<style>` element's sheet object stays the same while its text does;
//! a new text builds a new sheet. Rule objects are snapshots of the
//! sheet's rules. Editing through the CSSOM (`insertRule`, `style`
//! setters) and linked sheets' rules are not supported yet (TD-204).
//!
//! [CSSOM §6]: https://drafts.csswg.org/cssom/#css-object-model
//! [CSS Cascade 6 §3.5.3]: https://drafts.csswg.org/css-cascade-6/#the-cssscoperule-interface

use rustic_js::runtime::{Agent, JsResult, NativeFn, ObjectId, PropertyKey, Value};
use rustic_style::{CssomRule, CssomRuleKind, CssomSheet, document_style_owners};

use crate::heap::WrapperKind;
use crate::webidl::to_usv_string;

use super::events::{PrivateArray, define_method, with_root};
use super::{identity, illegal_constructor, install_interface, prototype, with_page, wrap};

/// Rule slots.
const TYPE: u32 = 0;
const TEXT: u32 = 1;
const PARENT_RULE: u32 = 2;
const PARENT_SHEET: u32 = 3;
const RULES: u32 = 4;
const STYLE: u32 = 5;
/// selectorText, name, keyText, conditionText, href, prefix.
const PRIMARY: u32 = 6;
/// start, layerName, namespaceURI.
const SECOND: u32 = 7;
/// end, supportsText.
const THIRD: u32 = 8;
/// media (a MediaList).
const MEDIA: u32 = 9;
/// nameList.
const NAMES: u32 = 10;
const RULE_SLOTS: usize = 11;

/// Sheet slots.
const SHEET_OWNER: u32 = 0;
const SHEET_RULES: u32 = 1;
const SHEET_MEDIA: u32 = 2;
const SHEET_TEXT: u32 = 3;

/// List slots (CSSRuleList, StyleSheetList, MediaList): `[items array]`
/// and, for a MediaList, `[items, mediaText]`.
const LIST_ITEMS: u32 = 0;
const LIST_TEXT: u32 = 1;

/// Declaration slots: `[names array, values array, priorities array, cssText]`.
const D_NAMES: u32 = 0;
const D_VALUES: u32 = 1;
const D_PRIORITIES: u32 = 2;
const D_TEXT: u32 = 3;

/// Page-lifetime CSSOM state: object → slots, and style element → sheet.
#[derive(Debug)]
pub(super) struct Cssom {
    records: ObjectId,
    sheets: ObjectId,
}

impl Cssom {
    pub(super) fn new(agent: &mut Agent) -> JsResult<Self> {
        let records = agent.new_weak_map()?;
        agent.root(Value::object(records))?;
        let sheets = agent.new_weak_map()?;
        agent.root(Value::object(sheets))?;
        Ok(Self { records, sheets })
    }
}

fn slot<const SLOT: u32>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let record = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.cssom.records, this))
    })?;
    match PrivateArray::from_value(record) {
        Some(record) => record.get(agent, SLOT),
        None => agent.type_error("invalid CSSOM receiver"),
    }
}

fn record(agent: &mut Agent, this: Value) -> JsResult<PrivateArray> {
    let record = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.cssom.records, this))
    })?;
    match PrivateArray::from_value(record) {
        Some(record) => Ok(record),
        None => agent.type_error("invalid CSSOM receiver"),
    }
}

fn list_length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let items = record(agent, this)?.get(agent, LIST_ITEMS)?;
    let items = PrivateArray::from_value(items).expect("list items");
    Ok(Value::number(f64::from(items.len(agent)?)))
}

fn list_item(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let items = record(agent, this)?.get(agent, LIST_ITEMS)?;
    let items = PrivateArray::from_value(items).expect("list items");
    let index = args.first().copied().unwrap_or(Value::UNDEFINED);
    let index = agent.to_number(index)?;
    let len = items.len(agent)?;
    if !(0.0..f64::from(len)).contains(&index.trunc()) {
        return Ok(Value::NULL);
    }
    items.get(agent, index.trunc() as u32)
}

fn declaration_value(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = record(agent, this)?;
    let wanted = args.first().copied().unwrap_or(Value::UNDEFINED);
    let wanted = to_usv_string(agent, wanted)?;
    let wanted = if wanted.starts_with("--") {
        wanted
    } else {
        wanted.to_ascii_lowercase()
    };
    let names = PrivateArray::from_value(record.get(agent, D_NAMES)?).expect("names");
    let values = PrivateArray::from_value(record.get(agent, D_VALUES)?).expect("values");
    let len = names.len(agent)?;
    for index in (0..len).rev() {
        let name = names.get(agent, index)?;
        if to_usv_string(agent, name)? == wanted {
            return values.get(agent, index);
        }
    }
    agent.string("")
}

fn declaration_priority(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = record(agent, this)?;
    let wanted = args.first().copied().unwrap_or(Value::UNDEFINED);
    let wanted = to_usv_string(agent, wanted)?.to_ascii_lowercase();
    let names = PrivateArray::from_value(record.get(agent, D_NAMES)?).expect("names");
    let priorities =
        PrivateArray::from_value(record.get(agent, D_PRIORITIES)?).expect("priorities");
    let len = names.len(agent)?;
    for index in (0..len).rev() {
        let name = names.get(agent, index)?;
        if to_usv_string(agent, name)?.to_ascii_lowercase() == wanted {
            return priorities.get(agent, index);
        }
    }
    agent.string("")
}

fn declaration_item(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let record = record(agent, this)?;
    let names = PrivateArray::from_value(record.get(agent, D_NAMES)?).expect("names");
    let index = args.first().copied().unwrap_or(Value::UNDEFINED);
    let index = agent.to_number(index)?.trunc();
    let len = names.len(agent)?;
    if !(0.0..f64::from(len)).contains(&index) {
        return agent.string("");
    }
    names.get(agent, index as u32)
}

fn declaration_length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let record = record(agent, this)?;
    let names = PrivateArray::from_value(record.get(agent, D_NAMES)?).expect("names");
    Ok(Value::number(f64::from(names.len(agent)?)))
}

const RULE_INTERFACES: [(&str, &str); 16] = [
    ("CSSStyleRule", "CSSGroupingRule"),
    ("CSSImportRule", "CSSRule"),
    ("CSSMediaRule", "CSSConditionRule"),
    ("CSSContainerRule", "CSSConditionRule"),
    ("CSSSupportsRule", "CSSConditionRule"),
    ("CSSKeyframesRule", "CSSRule"),
    ("CSSKeyframeRule", "CSSRule"),
    ("CSSLayerBlockRule", "CSSGroupingRule"),
    ("CSSLayerStatementRule", "CSSRule"),
    ("CSSScopeRule", "CSSGroupingRule"),
    ("CSSNestedDeclarations", "CSSRule"),
    ("CSSNamespaceRule", "CSSRule"),
    ("CSSFontFaceRule", "CSSRule"),
    ("CSSPropertyRule", "CSSRule"),
    ("CSSPageRule", "CSSGroupingRule"),
    ("CSSRuleBase", "CSSRule"),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        "StyleSheet",
        None,
        illegal_constructor,
        0,
        &[
            ("ownerNode", "get ownerNode", slot::<SHEET_OWNER>),
            ("media", "get media", slot::<SHEET_MEDIA>),
            ("type", "get type", sheet_type),
            ("href", "get href", null_getter),
            ("title", "get title", null_getter),
            ("parentStyleSheet", "get parentStyleSheet", null_getter),
            ("disabled", "get disabled", false_getter),
        ],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        "CSSStyleSheet",
        Some("StyleSheet"),
        illegal_constructor,
        0,
        &[
            ("cssRules", "get cssRules", slot::<SHEET_RULES>),
            ("rules", "get rules", slot::<SHEET_RULES>),
            ("ownerRule", "get ownerRule", null_getter),
        ],
        |_, _| Ok(()),
    )?;
    for interface in ["StyleSheetList", "CSSRuleList", "MediaList"] {
        install_interface(
            agent,
            interface,
            None,
            illegal_constructor,
            0,
            &[("length", "get length", list_length)],
            |agent, proto| define_method(agent, proto, "item", list_item, 1),
        )?;
    }
    let media = prototype(agent, "MediaList")?;
    let get = agent.new_native_function(slot::<LIST_TEXT>, "get mediaText", 0)?;
    let key = agent.heap.key("mediaText")?;
    agent.define_property_or_throw(
        media,
        key,
        rustic_js::runtime::PropertyDescriptor::accessor(
            Value::object(get),
            Value::UNDEFINED,
            true,
            true,
        ),
    )?;
    install_interface(
        agent,
        "CSSStyleProperties",
        Some("CSSStyleDeclaration"),
        illegal_constructor,
        0,
        &[
            ("cssText", "get cssText", slot::<D_TEXT>),
            ("length", "get length", declaration_length),
        ],
        |agent, proto| {
            for (name, method, length) in [
                ("getPropertyValue", declaration_value as NativeFn, 1),
                ("getPropertyPriority", declaration_priority, 1),
                ("item", declaration_item, 1),
            ] {
                define_method(agent, proto, name, method, length)?;
            }
            Ok(())
        },
    )?;
    install_interface(
        agent,
        "CSSRule",
        None,
        illegal_constructor,
        0,
        &[
            ("type", "get type", slot::<TYPE>),
            ("cssText", "get cssText", slot::<TEXT>),
            ("parentRule", "get parentRule", slot::<PARENT_RULE>),
            (
                "parentStyleSheet",
                "get parentStyleSheet",
                slot::<PARENT_SHEET>,
            ),
        ],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        "CSSGroupingRule",
        Some("CSSRule"),
        illegal_constructor,
        0,
        &[("cssRules", "get cssRules", slot::<RULES>)],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        "CSSConditionRule",
        Some("CSSGroupingRule"),
        illegal_constructor,
        0,
        &[("conditionText", "get conditionText", slot::<PRIMARY>)],
        |_, _| Ok(()),
    )?;
    for (interface, parent) in RULE_INTERFACES {
        let attributes: &[(&str, &str, NativeFn)] = match interface {
            "CSSStyleRule" => &[
                ("selectorText", "get selectorText", slot::<PRIMARY>),
                ("style", "get style", slot::<STYLE>),
            ],
            "CSSImportRule" => &[
                ("href", "get href", slot::<PRIMARY>),
                ("layerName", "get layerName", slot::<SECOND>),
                ("supportsText", "get supportsText", slot::<THIRD>),
                ("media", "get media", slot::<MEDIA>),
                ("styleSheet", "get styleSheet", null_getter),
            ],
            "CSSMediaRule" => &[("media", "get media", slot::<MEDIA>)],
            "CSSContainerRule" => &[("containerQuery", "get containerQuery", slot::<PRIMARY>)],
            "CSSKeyframesRule" => &[
                ("name", "get name", slot::<PRIMARY>),
                ("cssRules", "get cssRules", slot::<RULES>),
            ],
            "CSSKeyframeRule" => &[
                ("keyText", "get keyText", slot::<PRIMARY>),
                ("style", "get style", slot::<STYLE>),
            ],
            "CSSLayerBlockRule" => &[("name", "get name", slot::<PRIMARY>)],
            "CSSLayerStatementRule" => &[("nameList", "get nameList", slot::<NAMES>)],
            "CSSScopeRule" => &[
                ("start", "get start", slot::<SECOND>),
                ("end", "get end", slot::<THIRD>),
            ],
            "CSSNestedDeclarations" | "CSSFontFaceRule" => &[("style", "get style", slot::<STYLE>)],
            "CSSNamespaceRule" => &[
                ("prefix", "get prefix", slot::<PRIMARY>),
                ("namespaceURI", "get namespaceURI", slot::<SECOND>),
            ],
            "CSSPropertyRule" => &[("name", "get name", slot::<PRIMARY>)],
            "CSSPageRule" => &[("style", "get style", slot::<STYLE>)],
            _ => &[],
        };
        if interface == "CSSRuleBase" {
            continue;
        }
        install_interface(
            agent,
            interface,
            Some(parent),
            illegal_constructor,
            0,
            attributes,
            |_, _| Ok(()),
        )?;
    }
    let document = prototype(agent, "Document")?;
    let get = agent.new_native_function(style_sheets, "get styleSheets", 0)?;
    let key = agent.heap.key("styleSheets")?;
    agent.define_property_or_throw(
        document,
        key,
        rustic_js::runtime::PropertyDescriptor::accessor(
            Value::object(get),
            Value::UNDEFINED,
            true,
            true,
        ),
    )
}

fn null_getter(_: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::NULL)
}

fn false_getter(_: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::FALSE)
}

fn sheet_type(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    agent.string("text/css")
}

fn new_record(agent: &mut Agent, object: Value, slots: &[Value]) -> JsResult<PrivateArray> {
    let record = PrivateArray::new(agent, slots)?;
    with_page(agent, |agent, page| {
        agent.weak_map_set(page.cssom.records, object, record.value())
    })?;
    Ok(record)
}

/// A list object of `interface` holding `items`, readable by index.
fn new_list(
    agent: &mut Agent,
    interface: &'static str,
    items: &[Value],
    text: Option<&str>,
) -> JsResult<Value> {
    let proto = prototype(agent, interface)?;
    let object = agent.new_object_with(Some(proto))?;
    let value = Value::object(object);
    with_root(agent, value, |agent| {
        let array = PrivateArray::new(agent, items)?;
        let text = match text {
            Some(text) => agent.string(text)?,
            None => Value::UNDEFINED,
        };
        new_record(agent, value, &[array.value(), text])?;
        for (index, item) in items.iter().enumerate() {
            agent.create_data_property_or_throw(object, PropertyKey::Index(index as u32), *item)?;
        }
        Ok(value)
    })
}

fn media_list(agent: &mut Agent, text: &str) -> JsResult<Value> {
    let items: Vec<String> = text
        .split(',')
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect();
    let mut values = Vec::new();
    for item in &items {
        values.push(agent.string(item)?);
    }
    new_list(agent, "MediaList", &values, Some(&items.join(", ")))
}

fn declaration_object(agent: &mut Agent, rule: &CssomRule) -> JsResult<Value> {
    let proto = prototype(agent, "CSSStyleProperties")?;
    let object = Value::object(agent.new_object_with(Some(proto))?);
    with_root(agent, object, |agent| {
        let mut names = Vec::new();
        let mut values = Vec::new();
        let mut priorities = Vec::new();
        let mut text = Vec::new();
        for d in &rule.declarations {
            names.push(agent.string(&d.name)?);
            values.push(agent.string(&d.value)?);
            priorities.push(agent.string(if d.important { "important" } else { "" })?);
            text.push(format!(
                "{}: {}{};",
                d.name,
                d.value,
                if d.important { " !important" } else { "" }
            ));
        }
        let names = PrivateArray::new(agent, &names)?;
        let values = PrivateArray::new(agent, &values)?;
        let priorities = PrivateArray::new(agent, &priorities)?;
        let text = agent.string(&text.join(" "))?;
        new_record(
            agent,
            object,
            &[names.value(), values.value(), priorities.value(), text],
        )?;
        Ok(object)
    })
}

fn rule_object(
    agent: &mut Agent,
    rule: &CssomRule,
    parent_rule: Value,
    sheet: Value,
) -> JsResult<Value> {
    let proto = prototype(agent, rule.kind.interface())?;
    let object = Value::object(agent.new_object_with(Some(proto))?);
    with_root(agent, object, |agent| {
        let mut slots = vec![Value::NULL; RULE_SLOTS];
        slots[TYPE as usize] = Value::number(f64::from(rule.kind.legacy_type()));
        slots[TEXT as usize] = agent.string(&rule.text)?;
        slots[PARENT_RULE as usize] = parent_rule;
        slots[PARENT_SHEET as usize] = sheet;
        let string = |agent: &mut Agent, text: &str| agent.string(text);
        match &rule.kind {
            CssomRuleKind::Style { selector } => {
                slots[PRIMARY as usize] = string(agent, selector)?;
            }
            CssomRuleKind::Import {
                href,
                layer,
                supports,
                media,
            } => {
                slots[PRIMARY as usize] = string(agent, href)?;
                if let Some(layer) = layer {
                    slots[SECOND as usize] = string(agent, layer)?;
                }
                if let Some(supports) = supports {
                    slots[THIRD as usize] = string(agent, supports)?;
                }
                slots[MEDIA as usize] = media_list(agent, media)?;
            }
            CssomRuleKind::Media { condition } => {
                slots[PRIMARY as usize] = string(agent, condition)?;
                slots[MEDIA as usize] = media_list(agent, condition)?;
            }
            CssomRuleKind::Container { condition } | CssomRuleKind::Supports { condition } => {
                slots[PRIMARY as usize] = string(agent, condition)?;
            }
            CssomRuleKind::Keyframes { name } | CssomRuleKind::LayerBlock { name } => {
                slots[PRIMARY as usize] = string(agent, name)?;
            }
            CssomRuleKind::Property { name } => slots[PRIMARY as usize] = string(agent, name)?,
            CssomRuleKind::Keyframe { key } => slots[PRIMARY as usize] = string(agent, key)?,
            CssomRuleKind::LayerStatement { names } => {
                let mut values = Vec::new();
                for name in names {
                    values.push(string(agent, name)?);
                }
                let array = agent.create_array_from_list(&values)?;
                slots[NAMES as usize] = Value::object(array);
            }
            CssomRuleKind::Scope { start, end } => {
                if let Some(start) = start {
                    slots[SECOND as usize] = string(agent, start)?;
                }
                if let Some(end) = end {
                    slots[THIRD as usize] = string(agent, end)?;
                }
            }
            CssomRuleKind::Namespace { prefix, uri } => {
                slots[PRIMARY as usize] = string(agent, prefix)?;
                slots[SECOND as usize] = string(agent, uri)?;
            }
            CssomRuleKind::NestedDeclarations
            | CssomRuleKind::FontFace
            | CssomRuleKind::Page
            | CssomRuleKind::Other => {}
        }
        let record = new_record(agent, object, &slots)?;
        let style = declaration_object(agent, rule)?;
        record.set(agent, STYLE, style)?;
        let mut children = Vec::new();
        for child in &rule.rules {
            let child = rule_object(agent, child, object, sheet)?;
            children.push(child);
        }
        // Children are reachable from the list, which the record holds.
        let list = new_list(agent, "CSSRuleList", &children, None)?;
        record.set(agent, RULES, list)?;
        Ok(object)
    })
}

/// The sheet object of a `<style>` element with `text`, reused while the
/// text is unchanged.
fn sheet_for(agent: &mut Agent, owner: Value, text: &str, media: &str) -> JsResult<Value> {
    let cached = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.cssom.sheets, owner))
    })?;
    if let Some(sheet) = cached.as_object() {
        let record = record(agent, Value::object(sheet))?;
        let previous = record.get(agent, SHEET_TEXT)?;
        if previous.as_string().is_some() && to_usv_string(agent, previous)? == text {
            return Ok(Value::object(sheet));
        }
    }
    let proto = prototype(agent, "CSSStyleSheet")?;
    let sheet = Value::object(agent.new_object_with(Some(proto))?);
    with_root(agent, sheet, |agent| {
        let source = agent.string(text)?;
        let media = media_list(agent, media)?;
        let record = new_record(agent, sheet, &[owner, Value::NULL, media, source])?;
        let parsed = CssomSheet::parse(text);
        let mut rules = Vec::new();
        for rule in &parsed.rules {
            rules.push(rule_object(agent, rule, Value::NULL, sheet)?);
        }
        let list = new_list(agent, "CSSRuleList", &rules, None)?;
        record.set(agent, SHEET_RULES, list)?;
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.cssom.sheets, owner, sheet)
        })?;
        Ok(sheet)
    })
}

fn style_sheets(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let owners = with_page(agent, |agent, page| {
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        let id = identity(agent, &wrappers, this, WrapperKind::Node)?;
        page.read(agent, id.document(), |agent, document| {
            let mut out = Vec::new();
            for (node, is_style, text) in document_style_owners(document) {
                let media = rustic_dom_attribute(document, node, "media").unwrap_or_default();
                let wrapper = wrap(&mut wrappers, agent, document, node)?;
                let root = agent.root(wrapper)?;
                out.push((wrapper, root, is_style, text, media));
            }
            Ok(out)
        })
    })?;
    let mut sheets = Vec::new();
    let mut result = Ok(());
    for (owner, _, _, text, media) in &owners {
        match sheet_for(agent, *owner, text, media) {
            Ok(sheet) => sheets.push(sheet),
            Err(error) => {
                result = Err(error);
                break;
            }
        }
    }
    // Sheets hold their owners, so the owners' roots can go once the list
    // holds the sheets.
    let list = result.and_then(|()| new_list(agent, "StyleSheetList", &sheets, None));
    for (_, root, ..) in owners {
        agent.unroot(root);
    }
    list
}

fn rustic_dom_attribute(
    document: &rustic_dom::Document,
    node: rustic_dom::NodeId,
    name: &str,
) -> Option<String> {
    let rustic_dom::NodeKind::Element(element) = document.node(node).ok()?.kind() else {
        return None;
    };
    element
        .attributes()
        .iter()
        .find(|a| a.name().local() == name && a.name().namespace().is_none())
        .map(|a| String::from_utf16_lossy(a.value().as_units()))
}
