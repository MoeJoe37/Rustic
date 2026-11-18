//! Live, read-only computed declarations backed by one document layout snapshot.
//!
//! The weak-map association traces the element wrapper while the declaration
//! remains reachable, without keeping dead declarations or detached nodes alive.

use rustic_dom::NodeKind;
use rustic_js::runtime::{Abrupt, Agent, JsResult, NativeFn, PropertyDescriptor, Value};

use crate::dom_exception;

use crate::webidl::{IntegerConversion, to_unsigned_long};

use super::{
    ComputedStyleSnapshot, creation, events, illegal_constructor, install_interface, prototype,
    read_layout, require_receiver, required_arg, window, with_page,
};

const INTERFACE: &str = "CSSStyleDeclaration";

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        INTERFACE,
        None,
        illegal_constructor,
        0,
        &[("length", "get length", length)],
        |agent, proto| {
            let getter = Value::object(agent.new_native_function(css_text, "get cssText", 0)?);
            let setter = Value::object(agent.new_native_function(set_css_text, "set cssText", 1)?);
            let key = agent.heap.key("cssText")?;
            agent.define_property_or_throw(
                proto,
                key,
                PropertyDescriptor::accessor(getter, setter, true, true),
            )?;
            for (name, operation, count) in [
                ("getPropertyValue", get_property_value as NativeFn, 1),
                ("getPropertyPriority", get_property_priority, 1),
                ("item", item, 1),
                ("setProperty", set_property, 2),
                ("removeProperty", remove_property, 1),
            ] {
                events::define_method(agent, proto, name, operation, count)?;
            }
            Ok(())
        },
    )
}

/// The Window operation is installed on the realm global, matching other
/// Window methods. The result is live, not a copied property dictionary.
pub(super) fn get_computed_style(
    agent: &mut Agent,
    receiver: Value,
    args: &[Value],
) -> JsResult<Value> {
    window::receiver(agent, receiver)?;
    let element = required_arg(agent, args, 0)?;
    require_receiver(
        agent,
        element,
        |kind| matches!(kind, NodeKind::Element(_)),
        "expected an Element",
    )?;
    // A user-defined pseudo argument can run script and collect. Keep the
    // element alive until it is stored in the declaration's weak map.
    let root = agent.root(element)?;
    let result = (|| {
        if let Some(&pseudo) = args.get(1) {
            if !pseudo.is_nullish() {
                let value = creation::string(agent, pseudo)?;
                if !value.as_units().is_empty() {
                    return agent.type_error("computed pseudo-element styles are not supported");
                }
            }
        }
        let prototype = prototype(agent, INTERFACE)?;
        let declaration = Value::object(agent.new_object_with(Some(prototype))?);
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.computed_styles, declaration, element)?;
            Ok(declaration)
        })
    })();
    agent.unroot(root);
    result
}

fn element(agent: &mut Agent, declaration: Value) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        let element = agent.weak_map_get(page.computed_styles, declaration);
        if element == Value::UNDEFINED {
            return agent.type_error("expected a computed CSSStyleDeclaration");
        }
        Ok(element)
    })
}

fn snapshot(agent: &mut Agent, declaration: Value) -> JsResult<ComputedStyleSnapshot> {
    let element = element(agent, declaration)?;
    read_layout(agent, element, false, ComputedStyleSnapshot::default(), |tree, id| {
        tree.computed_style(id)
    })
}

fn value(agent: &mut Agent, text: &str) -> JsResult<Value> {
    Ok(Value::string(agent.heap.intern_str(text)?))
}

fn length(agent: &mut Agent, declaration: Value, _: &[Value]) -> JsResult<Value> {
    let style = snapshot(agent, declaration)?;
    let count = u32::try_from(style.len()).map_err(|_| Abrupt::OutOfMemory)?;
    Ok(Value::number(f64::from(count)))
}

fn get_property_value(agent: &mut Agent, declaration: Value, args: &[Value]) -> JsResult<Value> {
    // ToString may invoke a getter or callback. Finish before borrowing layout.
    let argument = required_arg(agent, args, 0)?;
    let property = creation::string(agent, argument)?;
    let name = creation::scalar_name(&property)?;
    let style = snapshot(agent, declaration)?;
    value(agent, name.as_deref().and_then(|name| style.get(name)).unwrap_or(""))
}

fn get_property_priority(
    agent: &mut Agent,
    declaration: Value,
    args: &[Value],
) -> JsResult<Value> {
    // Computed declarations have no author-specified priority.
    let argument = required_arg(agent, args, 0)?;
    let _ = creation::string(agent, argument)?;
    element(agent, declaration)?;
    value(agent, "")
}

fn item(agent: &mut Agent, declaration: Value, args: &[Value]) -> JsResult<Value> {
    let argument = required_arg(agent, args, 0)?;
    let index = to_unsigned_long(agent, argument, IntegerConversion::Wrap)?;
    let style = snapshot(agent, declaration)?;
    value(
        agent,
        style.name_at(usize::try_from(index).map_err(|_| Abrupt::OutOfMemory)?).unwrap_or(""),
    )
}

fn read_only(agent: &mut Agent) -> Abrupt {
    match dom_exception::new(
        agent,
        "NoModificationAllowedError",
        "computed style declarations are read-only",
    ) {
        Ok(error) => Abrupt::Throw(error),
        Err(error) => error,
    }
}

fn css_text(agent: &mut Agent, declaration: Value, _: &[Value]) -> JsResult<Value> {
    element(agent, declaration)?;
    value(agent, "")
}

fn set_css_text(agent: &mut Agent, declaration: Value, args: &[Value]) -> JsResult<Value> {
    let argument = required_arg(agent, args, 0)?;
    let _ = creation::string(agent, argument)?;
    element(agent, declaration)?;
    Err(read_only(agent))
}

fn set_property(agent: &mut Agent, declaration: Value, args: &[Value]) -> JsResult<Value> {
    let name = required_arg(agent, args, 0)?;
    let _ = creation::string(agent, name)?;
    if let Some(&value) = args.get(1) {
        if !value.is_nullish() {
            let _ = creation::string(agent, value)?;
        }
    }
    if let Some(&priority) = args.get(2) {
        let _ = creation::string(agent, priority)?;
    }
    element(agent, declaration)?;
    Err(read_only(agent))
}

fn remove_property(agent: &mut Agent, declaration: Value, args: &[Value]) -> JsResult<Value> {
    let name = required_arg(agent, args, 0)?;
    let _ = creation::string(agent, name)?;
    element(agent, declaration)?;
    Err(read_only(agent))
}
