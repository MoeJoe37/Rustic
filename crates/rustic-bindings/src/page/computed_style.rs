//! Live, read-only computed declarations backed by one document layout snapshot.
//!
//! The weak-map association traces the element wrapper while the declaration
//! remains reachable, without keeping dead declarations or detached nodes alive.

use rustic_css::PropertyId;
use rustic_dom::NodeKind;
use rustic_js::runtime::{
    Abrupt, Agent, JsResult, LiveIndexAccess, NativeFn, ObjectId, PropertyDescriptor, PropertyKey,
    Value,
};

use crate::dom_exception;

use crate::webidl::{IntegerConversion, to_unsigned_long};

use super::{
    ComputedStyleSnapshot, creation, events, illegal_constructor, install_interface, prototype,
    read_layout, require_receiver, required_arg, window, with_page,
};

const INTERFACE: &str = "CSSStyleDeclaration";

static STYLE_INDEX: LiveIndexAccess = LiveIndexAccess {
    length: indexed_length,
    item: indexed_item,
    named: None,
};

fn indexed_length(agent: &mut Agent, source: Value) -> JsResult<u32> {
    u32::try_from(snapshot_from_element(agent, source)?.len()).map_err(|_| Abrupt::OutOfMemory)
}

fn indexed_item(agent: &mut Agent, source: Value, index: u32) -> JsResult<Option<Value>> {
    let style = snapshot_from_element(agent, source)?;
    style
        .name_at(index as usize)
        .map(|name| value(agent, name))
        .transpose()
}

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
    get_longhand::<110> as NativeFn,
    get_longhand::<111> as NativeFn,
    get_longhand::<112> as NativeFn,
    get_longhand::<113> as NativeFn,
    get_longhand::<114> as NativeFn,
    get_longhand::<115> as NativeFn,
    get_longhand::<116> as NativeFn,
    get_longhand::<117> as NativeFn,
    get_longhand::<118> as NativeFn,
    get_longhand::<119> as NativeFn,
    get_longhand::<120> as NativeFn,
    get_longhand::<121> as NativeFn,
    get_longhand::<122> as NativeFn,
    get_longhand::<123> as NativeFn,
    get_longhand::<124> as NativeFn,
    get_longhand::<125> as NativeFn,
    get_longhand::<126> as NativeFn,
    get_longhand::<127> as NativeFn,
    get_longhand::<128> as NativeFn,
    get_longhand::<129> as NativeFn,
    get_longhand::<130> as NativeFn,
    get_longhand::<131> as NativeFn,
    get_longhand::<132> as NativeFn,
    get_longhand::<133> as NativeFn,
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

fn install_longhand_getters(agent: &mut Agent, prototype: ObjectId) -> JsResult<()> {
    // The interface prototype is not installed on the realm until this
    // initializer returns; protect it throughout the allocating loop.
    let prototype_root = agent.root(Value::object(prototype))?;
    let result = (|| {
        let setter = Value::object(agent.new_native_function(
            set_named_longhand,
            "set computed property",
            1,
        )?);
        let setter_root = agent.root(setter)?;
        let result = (|| {
            for (&property, behavior) in PropertyId::ALL.iter().zip(LONGHAND_GETTERS) {
                let canonical = property.name();
                let camel = camel_name(canonical);
                let getter = Value::object(agent.new_native_function(
                    behavior,
                    &format!("get {camel}"),
                    0,
                )?);
                let getter_root = agent.root(getter)?;
                let installed: JsResult<()> = (|| {
                    for (index, name) in [canonical, camel.as_str()].into_iter().enumerate() {
                        // A one-word longhand has identical CSS and IDL spellings:
                        // install it once, rather than accidentally skipping both.
                        if index == 1 && canonical == camel {
                            continue;
                        }
                        let key = agent.heap.key(name)?;
                        agent.define_property_or_throw(
                            prototype,
                            key,
                            PropertyDescriptor::accessor(getter, setter, true, true),
                        )?;
                    }
                    Ok(())
                })();
                agent.unroot(getter_root);
                installed?;
            }
            Ok(())
        })();
        agent.unroot(setter_root);
        result
    })();
    agent.unroot(prototype_root);
    result
}

fn set_named_longhand(agent: &mut Agent, declaration: Value, args: &[Value]) -> JsResult<Value> {
    element(agent, declaration)?;
    let argument = required_arg(agent, args, 0)?;
    let _ = creation::string(agent, argument)?;
    Err(read_only(agent))
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
            agent.define_property_or_throw(
                proto,
                PropertyKey::Symbol(agent.symbols.iterator),
                PropertyDescriptor::data_with(
                    Value::object(agent.intrinsics.array_values),
                    true,
                    false,
                    true,
                ),
            )?;
            let getter = Value::object(agent.new_native_function(css_text, "get cssText", 0)?);
            let setter =
                Value::object(agent.new_native_function(set_css_text, "set cssText", 1)?);
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
                if value.as_units().first() == Some(&u16::from(b':')) {
                    return agent.type_error("computed pseudo-element styles are not supported");
                }
            }
        }
        let prototype = prototype(agent, INTERFACE)?;
        // The live indexed host traces its source element and implements
        // own-key discovery, descriptors and read-only numeric indices.
        let declaration =
            Value::object(agent.new_indexed_view(INTERFACE, prototype, element, &STYLE_INDEX)?);
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

fn snapshot_from_element(agent: &mut Agent, element: Value) -> JsResult<ComputedStyleSnapshot> {
    read_layout(
        agent,
        element,
        false,
        ComputedStyleSnapshot::default(),
        |tree, id| tree.computed_style(id),
    )
}

fn snapshot(agent: &mut Agent, declaration: Value) -> JsResult<ComputedStyleSnapshot> {
    let source = element(agent, declaration)?;
    snapshot_from_element(agent, source)
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
    // Brand-check before invoking author code, then read layout after conversion.
    let source = element(agent, declaration)?;
    let argument = required_arg(agent, args, 0)?;
    let property = creation::string(agent, argument)?;
    let name = creation::scalar_name(&property)?;
    let style = snapshot_from_element(agent, source)?;
    let Some(name) = name.as_deref() else {
        return value(agent, "");
    };
    if let Some(longhand) = style.get(name) {
        return value(agent, longhand);
    }
    let shorthand = rustic_css::serialize_four_sided_shorthand(name, |id| style.get_longhand(id));
    value(agent, shorthand.as_deref().unwrap_or(""))
}

fn get_property_priority(agent: &mut Agent, declaration: Value, args: &[Value]) -> JsResult<Value> {
    // Computed declarations have no author-specified priority.
    element(agent, declaration)?;
    let argument = required_arg(agent, args, 0)?;
    let _ = creation::string(agent, argument)?;
    value(agent, "")
}

fn item(agent: &mut Agent, declaration: Value, args: &[Value]) -> JsResult<Value> {
    let source = element(agent, declaration)?;
    let argument = required_arg(agent, args, 0)?;
    let index = to_unsigned_long(agent, argument, IntegerConversion::Wrap)?;
    let style = snapshot_from_element(agent, source)?;
    value(
        agent,
        style
            .name_at(usize::try_from(index).map_err(|_| Abrupt::OutOfMemory)?)
            .unwrap_or(""),
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
    element(agent, declaration)?;
    let argument = required_arg(agent, args, 0)?;
    let _ = creation::string(agent, argument)?;
    Err(read_only(agent))
}

fn set_property(agent: &mut Agent, declaration: Value, args: &[Value]) -> JsResult<Value> {
    element(agent, declaration)?;
    let value = required_arg(agent, args, 1)?;
    let _ = creation::string(agent, args[0])?;
    if !value.is_nullish() {
        let _ = creation::string(agent, value)?;
    }
    if let Some(&priority) = args.get(2) {
        let _ = creation::string(agent, priority)?;
    }
    Err(read_only(agent))
}

fn remove_property(agent: &mut Agent, declaration: Value, args: &[Value]) -> JsResult<Value> {
    element(agent, declaration)?;
    let name = required_arg(agent, args, 0)?;
    let _ = creation::string(agent, name)?;
    Err(read_only(agent))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn installed_longhand_getters_include_identical_css_and_camel_names() {
        let mut agent = Agent::new().expect("agent");
        install(&mut agent).expect("CSSStyleDeclaration interface");
        let prototype = agent
            .host_interface_prototype(INTERFACE)
            .expect("installed prototype");

        // Regression: an earlier alias-deduplication loop skipped both names
        // when the CSS and camel-case spellings were identical.
        for name in [
            "color",
            "display",
            "width",
            "background-color",
            "backgroundColor",
            "cssFloat",
        ] {
            let key = agent.heap.key(name).expect("property name");
            let descriptor = agent
                .get_own_property(prototype, key)
                .expect("property lookup")
                .unwrap_or_else(|| panic!("missing longhand getter: {name}"));
            assert!(
                descriptor.get.and_then(Value::as_object).is_some(),
                "{name}"
            );
            assert!(
                descriptor.set.and_then(Value::as_object).is_some(),
                "{name}"
            );
        }
    }

    #[test]
    fn longhand_getter_names_cover_the_supported_catalog_without_collisions() {
        assert_eq!(LONGHAND_GETTERS.len(), PropertyId::ALL.len());
        let mut seen = HashSet::new();
        for &property in PropertyId::ALL {
            let alias = camel_name(property.name());
            assert!(
                seen.insert(alias.clone()),
                "duplicate named getter: {alias}"
            );
        }
        assert_eq!(camel_name("background-color"), "backgroundColor");
        assert_eq!(camel_name("border-top-left-radius"), "borderTopLeftRadius");
        assert_eq!(camel_name("float"), "cssFloat");
    }
}
