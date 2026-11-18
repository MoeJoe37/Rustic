//! Live, read-only computed declarations backed by one document layout snapshot.
//!
//! The weak-map association traces the element wrapper while the declaration
//! remains reachable, without keeping dead declarations or detached nodes alive.

use rustic_css::PropertyId;
use rustic_dom::NodeKind;
use rustic_js::runtime::{Abrupt, Agent, JsResult, NativeFn, PropertyDescriptor, Value};

use crate::dom_exception;

use crate::webidl::{IntegerConversion, to_unsigned_long};

use super::{
    ComputedStyleSnapshot, creation, events, illegal_constructor, install_interface, prototype,
    read_layout, require_receiver, required_arg, window, with_page,
};

const INTERFACE: &str = "CSSStyleDeclaration";

 // Every longhand has the same bounded snapshot read. A new supported
 // property makes the getter table fail to compile until it is included.
 const LONGHAND_GETTERS: [NativeFn; PropertyId::ALL.len()] = [
    get_longhand::<0> as NativeFn,
    get_longhand::<1> as NativeFn,
    get_longhand::<2> as NativeFn,
    get_longhand::<3> as NativeFn,
    get_longhand::<4> as NativeFn,
    get_longhand::<5> as NativeFn,
    get_longhand::<6> as NativeFn,
    get_longhand::<7> as NativeFn,
    get_longhand::<8> as NativeFn,
    get_longhand::<9> as NativeFn,
    get_longhand::<10> as NativeFn,
    get_longhand::<11> as NativeFn,
    get_longhand::<12> as NativeFn,
    get_longhand::<13> as NativeFn,
    get_longhand::<14> as NativeFn,
    get_longhand::<15> as NativeFn,
    get_longhand::<16> as NativeFn,
    get_longhand::<17> as NativeFn,
    get_longhand::<18> as NativeFn,
    get_longhand::<19> as NativeFn,
    get_longhand::<20> as NativeFn,
    get_longhand::<21> as NativeFn,
    get_longhand::<22> as NativeFn,
    get_longhand::<23> as NativeFn,
    get_longhand::<24> as NativeFn,
    get_longhand::<25> as NativeFn,
    get_longhand::<26> as NativeFn,
    get_longhand::<27> as NativeFn,
    get_longhand::<28> as NativeFn,
    get_longhand::<29> as NativeFn,
    get_longhand::<30> as NativeFn,
    get_longhand::<31> as NativeFn,
    get_longhand::<32> as NativeFn,
    get_longhand::<33> as NativeFn,
    get_longhand::<34> as NativeFn,
    get_longhand::<35> as NativeFn,
    get_longhand::<36> as NativeFn,
    get_longhand::<37> as NativeFn,
    get_longhand::<38> as NativeFn,
    get_longhand::<39> as NativeFn,
    get_longhand::<40> as NativeFn,
    get_longhand::<41> as NativeFn,
    get_longhand::<42> as NativeFn,
    get_longhand::<43> as NativeFn,
    get_longhand::<44> as NativeFn,
    get_longhand::<45> as NativeFn,
    get_longhand::<46> as NativeFn,
    get_longhand::<47> as NativeFn,
    get_longhand::<48> as NativeFn,
    get_longhand::<49> as NativeFn,
    get_longhand::<50> as NativeFn,
    get_longhand::<51> as NativeFn,
    get_longhand::<52> as NativeFn,
    get_longhand::<53> as NativeFn,
    get_longhand::<54> as NativeFn,
    get_longhand::<55> as NativeFn,
    get_longhand::<56> as NativeFn,
    get_longhand::<57> as NativeFn,
    get_longhand::<58> as NativeFn,
    get_longhand::<59> as NativeFn,
    get_longhand::<60> as NativeFn,
    get_longhand::<61> as NativeFn,
    get_longhand::<62> as NativeFn,
    get_longhand::<63> as NativeFn,
    get_longhand::<64> as NativeFn,
    get_longhand::<65> as NativeFn,
    get_longhand::<66> as NativeFn,
    get_longhand::<67> as NativeFn,
    get_longhand::<68> as NativeFn,
    get_longhand::<69> as NativeFn,
    get_longhand::<70> as NativeFn,
    get_longhand::<71> as NativeFn,
    get_longhand::<72> as NativeFn,
    get_longhand::<73> as NativeFn,
    get_longhand::<74> as NativeFn,
    get_longhand::<75> as NativeFn,
    get_longhand::<76> as NativeFn,
    get_longhand::<77> as NativeFn,
    get_longhand::<78> as NativeFn,
    get_longhand::<79> as NativeFn,
    get_longhand::<80> as NativeFn,
    get_longhand::<81> as NativeFn,
    get_longhand::<82> as NativeFn,
    get_longhand::<83> as NativeFn,
    get_longhand::<84> as NativeFn,
    get_longhand::<85> as NativeFn,
    get_longhand::<86> as NativeFn,
    get_longhand::<87> as NativeFn,
    get_longhand::<88> as NativeFn,
    get_longhand::<89> as NativeFn,
    get_longhand::<90> as NativeFn,
    get_longhand::<91> as NativeFn,
    get_longhand::<92> as NativeFn,
    get_longhand::<93> as NativeFn,
    get_longhand::<94> as NativeFn,
    get_longhand::<95> as NativeFn,
    get_longhand::<96> as NativeFn,
    get_longhand::<97> as NativeFn,
    get_longhand::<98> as NativeFn,
    get_longhand::<99> as NativeFn,
    get_longhand::<100> as NativeFn,
    get_longhand::<101> as NativeFn,
    get_longhand::<102> as NativeFn,
    get_longhand::<103> as NativeFn,
    get_longhand::<104> as NativeFn,
    get_longhand::<105> as NativeFn,
    get_longhand::<106> as NativeFn,
    get_longhand::<107> as NativeFn,
    get_longhand::<108> as NativeFn,
    get_longhand::<109> as NativeFn,
 ];

 fn get_longhand<const INDEX: usize>(
     agent: &mut Agent,
     declaration: Value,
     _: &[Value],
 ) -> JsResult<Value> {
     let style = snapshot(agent, declaration)?;
     let name = PropertyId::ALL[INDEX].name();
     value(agent, style.get(name).unwrap_or(""))
 }

 fn camel_name(name: &str) -> String {
     if name == "float" {
         return String::from("cssFloat");
     }
     let mut camel = String::with_capacity(name.len());
     let mut upper = false;
     for ch in name.chars() {
         if ch == '-' {
             upper = true;
         } else {
             camel.push(if upper { ch.to_ascii_uppercase() } else { ch });
             upper = false;
         }
     }
     camel
 }

 fn install_longhand_getters(
     agent: &mut Agent,
     prototype: rustic_js::runtime::ObjectId,
 ) -> JsResult<()> {
     for (&property, behavior) in PropertyId::ALL.iter().zip(LONGHAND_GETTERS) {
         let canonical = property.name();
         let camel = camel_name(canonical);
         let getter = Value::object(
             agent.new_native_function(behavior, &format!("get {camel}"), 0)?,
         );
         for name in [canonical, camel.as_str()] {
             if name == canonical && canonical == camel {
                 continue;
             }
             let key = agent.heap.key(name)?;
             agent.define_property_or_throw(
                 prototype,
                 key,
                 PropertyDescriptor::accessor(getter, Value::UNDEFINED, true, true),
             )?;
         }
     }
     Ok(())
 }


pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        INTERFACE,
        None,
        illegal_constructor,
        0,
        &[("length", "get length", length)],
        |agent, proto| {
            install_longhand_getters(agent, proto)?;
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
        // The weak-map write may allocate; keep its key rooted until insertion.
        let declaration_root = agent.root(declaration)?;
        let result = with_page(agent, |agent, page| {
            agent.weak_map_set(page.computed_styles, declaration, element)?;
            Ok(declaration)
        });
        agent.unroot(declaration_root);
        result
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
