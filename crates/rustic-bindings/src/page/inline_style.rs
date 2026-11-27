//! `element.style` (CSSOM §6.7: ElementCSSInlineStyle).
//!
//! The `style` attribute is the declarations' only store: every read parses
//! it ([`rustic_style::parse_declaration_block`]) and every change writes it
//! back serialized, so the cascade, mutation observers and `getAttribute`
//! see one value. The declaration objects share the `CSSStyleDeclaration`
//! prototype with computed styles; its operations dispatch on the receiver
//! ([`owner`]). Shorthands are stored as written rather than expanded into
//! longhands (TD-214).

use rustic_dom::{DomString, NodeKind, QualifiedName};
use rustic_js::runtime::{Agent, JsResult, LiveIndexAccess, Value};
use rustic_style::{
    CssomDeclaration, parse_declaration_block, parse_property_value, serialize_declaration_block,
};

use super::{
    creation, dom_exception, prototype, read_node, require_receiver, required_arg, with_mut_tree,
    with_page,
};

const INTERFACE: &str = "CSSStyleDeclaration";

static INLINE_INDEX: LiveIndexAccess = LiveIndexAccess {
    length: indexed_length,
    item: indexed_item,
    named: None,
};

fn indexed_length(agent: &mut Agent, element: Value) -> JsResult<u32> {
    Ok(read(agent, element)?.len() as u32)
}

fn indexed_item(agent: &mut Agent, element: Value, index: u32) -> JsResult<Option<Value>> {
    let declarations = read(agent, element)?;
    declarations
        .get(index as usize)
        .map(|d| text(agent, &d.name))
        .transpose()
}

fn text(agent: &mut Agent, value: &str) -> JsResult<Value> {
    Ok(Value::string(agent.heap.intern_str(value)?))
}

/// The element owning `declaration` when it is an inline style.
pub(super) fn owner(agent: &mut Agent, declaration: Value) -> JsResult<Option<Value>> {
    with_page(agent, |agent, page| {
        let element = agent.weak_map_get(page.inline_styles, declaration);
        Ok((element != Value::UNDEFINED).then_some(element))
    })
}

/// `element.style`: one declaration object per element.
pub(super) fn style(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    require_receiver(
        agent,
        this,
        |kind| matches!(kind, NodeKind::Element(_)),
        "expected an Element receiver",
    )?;
    let existing = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.element_styles, this))
    })?;
    if existing != Value::UNDEFINED {
        return Ok(existing);
    }
    let root = agent.root(this)?;
    let result = (|| {
        let proto = prototype(agent, INTERFACE)?;
        let declaration =
            Value::object(agent.new_indexed_view(INTERFACE, proto, this, &INLINE_INDEX)?);
        let declaration_root = agent.root(declaration)?;
        let stored = with_page(agent, |agent, page| {
            agent.weak_map_set(page.inline_styles, declaration, this)?;
            agent.weak_map_set(page.element_styles, this, declaration)
        });
        agent.unroot(declaration_root);
        stored.map(|()| declaration)
    })();
    agent.unroot(root);
    result
}

/// `element.style = text` (`[PutForwards=cssText]`).
pub(super) fn set_style(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let declaration = style(agent, this, &[])?;
    let value = required_arg(agent, args, 0)?;
    set_css_text(agent, declaration, value)?;
    Ok(Value::UNDEFINED)
}

/// The element's current declarations.
pub(super) fn read(agent: &mut Agent, element: Value) -> JsResult<Vec<CssomDeclaration>> {
    let source = read_node(agent, element, |_, _, _, _, node| {
        Ok(match node.kind() {
            NodeKind::Element(element) => element
                .attributes()
                .iter()
                .find(|a| a.name().namespace().is_none() && a.name().local() == "style")
                .map(|a| String::from_utf16_lossy(a.value().as_units())),
            _ => None,
        })
    })?;
    Ok(source
        .map(|s| parse_declaration_block(&s))
        .unwrap_or_default())
}

fn write(agent: &mut Agent, element: Value, declarations: &[CssomDeclaration]) -> JsResult<()> {
    let serialized = serialize_declaration_block(declarations);
    with_mut_tree(agent, element, |agent, document, _, id| {
        let name = QualifiedName::from_parsed(None, None, "style")
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        document
            .set_attribute(id, name, DomString::from(serialized.as_str()))
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        Ok(())
    })
}

/// A property name as CSSOM compares it: custom properties as written,
/// others ASCII-lowercased.
fn normalized(name: &str) -> String {
    if name.starts_with("--") {
        name.to_owned()
    } else {
        name.to_ascii_lowercase()
    }
}

pub(super) fn length(agent: &mut Agent, element: Value) -> JsResult<Value> {
    Ok(Value::number(read(agent, element)?.len() as f64))
}

pub(super) fn item(agent: &mut Agent, element: Value, index: u32) -> JsResult<Value> {
    let declarations = read(agent, element)?;
    let name = declarations
        .get(index as usize)
        .map_or("", |d| d.name.as_str())
        .to_owned();
    text(agent, &name)
}

/// `getPropertyValue(name)`, with four-sided shorthands composed from
/// their longhands.
pub(super) fn property_value(agent: &mut Agent, element: Value, name: &str) -> JsResult<Value> {
    let name = normalized(name);
    let declarations = read(agent, element)?;
    if let Some(found) = declarations.iter().find(|d| d.name == name) {
        let value = found.value.clone();
        return text(agent, &value);
    }
    let shorthand = rustic_css::serialize_four_sided_shorthand(&name, |id| {
        declarations
            .iter()
            .find(|d| d.name == id.name())
            .map(|d| d.value.as_str())
    });
    text(agent, shorthand.as_deref().unwrap_or(""))
}

pub(super) fn property_priority(agent: &mut Agent, element: Value, name: &str) -> JsResult<Value> {
    let name = normalized(name);
    let important = read(agent, element)?
        .iter()
        .any(|d| d.name == name && d.important);
    text(agent, if important { "important" } else { "" })
}

/// `setProperty(name, value, priority)` (CSSOM §6.6.1).
pub(super) fn set_property(
    agent: &mut Agent,
    element: Value,
    name: &str,
    value: &str,
    priority: &str,
) -> JsResult<()> {
    let name = normalized(name);
    if value.is_empty() {
        remove_property(agent, element, &name)?;
        return Ok(());
    }
    let important = if priority.is_empty() {
        false
    } else if priority.eq_ignore_ascii_case("important") {
        true
    } else {
        return Ok(());
    };
    let Some(value) = parse_property_value(&name, value) else {
        return Ok(());
    };
    let mut declarations = read(agent, element)?;
    match declarations.iter_mut().find(|d| d.name == name) {
        Some(existing) => {
            existing.value = value;
            existing.important = important;
        }
        None => declarations.push(CssomDeclaration {
            name,
            value,
            important,
        }),
    }
    write(agent, element, &declarations)
}

/// `removeProperty(name)`: the removed value.
pub(super) fn remove_property(agent: &mut Agent, element: Value, name: &str) -> JsResult<Value> {
    let name = normalized(name);
    let mut declarations = read(agent, element)?;
    let Some(index) = declarations.iter().position(|d| d.name == name) else {
        return text(agent, "");
    };
    let removed = declarations.remove(index);
    write(agent, element, &declarations)?;
    text(agent, &removed.value)
}

pub(super) fn css_text(agent: &mut Agent, element: Value) -> JsResult<Value> {
    let serialized = serialize_declaration_block(&read(agent, element)?);
    text(agent, &serialized)
}

fn set_css_text(agent: &mut Agent, declaration: Value, value: Value) -> JsResult<()> {
    let Some(element) = owner(agent, declaration)? else {
        return agent.type_error("expected an inline CSSStyleDeclaration");
    };
    let source = creation::string(agent, value)?;
    let source = String::from_utf16_lossy(source.as_units());
    write(agent, element, &parse_declaration_block(&source))
}

/// The `cssText` setter for inline declarations.
pub(super) fn assign_css_text(agent: &mut Agent, declaration: Value, value: Value) -> JsResult<()> {
    set_css_text(agent, declaration, value)
}
