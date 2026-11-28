//! ElementInternals state for custom elements and form participation.

use rustic_dom::NodeKind;
use rustic_js::runtime::{Agent, JsResult, Value};

use super::super::events::{PrivateArray, define_method, with_root};
use super::super::{
    dom_exception, install_interface, prototype, read_node, required_arg, with_mut_tree, wrap,
};
use super::{Definition, call, creation, current_definition, with_page};

const INTERFACE: &str = "ElementInternals";
pub(in crate::page) const FLAGS: [&str; 10] = [
    "badInput",
    "customError",
    "patternMismatch",
    "rangeOverflow",
    "rangeUnderflow",
    "stepMismatch",
    "tooLong",
    "tooShort",
    "typeMismatch",
    "valueMissing",
];

const FORM: u32 = 0;
const DISABLED: u32 = 1;
const SUBMISSION: u32 = 2;
const STATE: u32 = 3;
const VALIDITY: u32 = 4;
const MESSAGE: u32 = 5;
const ANCHOR: u32 = 6;
const LABELS: u32 = 7;

#[derive(Debug)]
pub(super) struct Forms {
    pub associated: Value,
    pub disabled: Value,
    pub reset: Value,
    pub restore: Value,
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        INTERFACE,
        None,
        super::super::illegal_constructor,
        0,
        &[
            ("shadowRoot", "get shadowRoot", shadow_root),
            ("form", "get form", form),
            ("willValidate", "get willValidate", will_validate),
            ("validity", "get validity", validity),
            (
                "validationMessage",
                "get validationMessage",
                validation_message,
            ),
            ("labels", "get labels", labels),
        ],
        |agent, proto| {
            define_method(agent, proto, "setFormValue", set_form_value, 1)?;
            define_method(agent, proto, "setValidity", set_validity, 1)?;
            define_method(agent, proto, "checkValidity", check_validity, 0)?;
            define_method(agent, proto, "reportValidity", check_validity, 0)
        },
    )?;
    let html = prototype(agent, "HTMLElement")?;
    define_method(agent, html, "attachInternals", attach, 0)
}

fn unsupported(agent: &mut Agent, reason: &str) -> rustic_js::runtime::Abrupt {
    creation::exception(agent, "NotSupportedError", reason)
}

fn attach(agent: &mut Agent, target: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, target, |agent, _, _, _, node| {
        if !matches!(node.kind(), NodeKind::Element(e) if e.name().namespace() == Some(rustic_dom::HTML_NAMESPACE))
        {
            return agent.type_error("expected an HTMLElement receiver");
        }
        if matches!(node.kind(), NodeKind::Element(e) if e.custom_is().is_some()) {
            return Err(unsupported(
                agent,
                "customized built-ins cannot attach internals",
            ));
        }
        Ok(())
    })?;
    with_page(agent, |agent, page| {
        let registry = &page.custom_elements;
        let definition = current_definition(agent, registry, target)
            .ok_or_else(|| unsupported(agent, "element is not a constructed custom element"))?;
        if definition.disable_internals {
            return Err(unsupported(
                agent,
                "custom element definition disables internals",
            ));
        }
        if agent.weak_map_has(registry.attached, target) {
            return Err(unsupported(agent, "element already has attached internals"));
        }
        let proto = prototype(agent, INTERFACE)?;
        let internals = Value::object(agent.new_object_with(Some(proto))?);
        with_root(agent, internals, |agent| {
            agent.weak_map_set(registry.internals_targets, internals, target)?;
            agent.weak_map_set(registry.attached, target, internals)?;
            Ok(internals)
        })
    })
}

fn target(agent: &mut Agent, internals: Value) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        let target = agent.weak_map_get(page.custom_elements.internals_targets, internals);
        if target == Value::UNDEFINED {
            return agent.type_error("expected an ElementInternals receiver");
        }
        Ok(target)
    })
}

fn form_target(agent: &mut Agent, internals: Value) -> JsResult<Value> {
    let target = target(agent, internals)?;
    with_page(agent, |agent, page| {
        if current_definition(agent, &page.custom_elements, target)
            .is_none_or(|def| def.forms.is_none())
        {
            return Err(unsupported(agent, "element is not form-associated"));
        }
        Ok(target)
    })
}

fn shadow_root(agent: &mut Agent, internals: Value, _: &[Value]) -> JsResult<Value> {
    let target = target(agent, internals)?;
    read_node(agent, target, |agent, wrappers, document, _, node| {
        let NodeKind::Element(element) = node.kind() else {
            unreachable!("internals target")
        };
        if let Some(root) = element.shadow_root().filter(|_| {
            element
                .shadow_root_options()
                .is_some_and(|options| options.available_to_internals)
        }) {
            return wrap(wrappers, agent, document, root);
        }
        Ok(Value::NULL)
    })
}

pub(super) fn mark_associated(agent: &mut Agent, target: Value) -> JsResult<()> {
    with_mut_tree(agent, target, |agent, document, _, id| {
        document
            .mark_form_associated(id)
            .map_err(|error| dom_exception::from_dom_error(agent, error))
    })
}

fn data(agent: &mut Agent, target: Value) -> JsResult<PrivateArray> {
    with_page(agent, |agent, page| {
        let map = page.custom_elements.form_data;
        if let Some(data) = PrivateArray::from_value(agent.weak_map_get(map, target)) {
            return Ok(data);
        }
        let state = PrivateArray::new(
            agent,
            &[
                Value::NULL,
                Value::FALSE,
                Value::NULL,
                Value::NULL,
                Value::number(0.0),
                Value::UNDEFINED,
                Value::NULL,
                Value::UNDEFINED,
            ],
        )?;
        agent.weak_map_set(map, target, state.value())?;
        Ok(state)
    })
}

pub(super) fn refresh(agent: &mut Agent, target: Value, definition: &Definition) -> JsResult<()> {
    let Some(callbacks) = definition.forms.as_ref() else {
        return Ok(());
    };
    if callbacks.associated == Value::UNDEFINED && callbacks.disabled == Value::UNDEFINED {
        return Ok(());
    }
    let (form, disabled) = super::super::events::custom_form_status(agent, target)?;
    let data = data(agent, target)?;
    let before = data.get(agent, FORM)?;
    let old_disabled = data.get(agent, DISABLED)?;
    let disabled = Value::boolean(disabled);
    data.set(agent, FORM, form)?;
    data.set(agent, DISABLED, disabled)?;
    if before != form {
        call(agent, callbacks.associated, target, &[form])?;
    }
    if old_disabled != disabled && data.get(agent, DISABLED)? == disabled {
        call(agent, callbacks.disabled, target, &[disabled])?;
    }
    Ok(())
}

pub(in crate::page) fn reset(agent: &mut Agent, target: Value) -> JsResult<()> {
    with_page(agent, |agent, page| {
        if let Some(definition) = super::upgraded(agent, &page.custom_elements, target) {
            if let Some(forms) = definition.forms.as_ref() {
                call(agent, forms.reset, target, &[])?;
            }
        }
        Ok(())
    })
}

pub(in crate::page) fn submission(agent: &mut Agent, target: Value) -> JsResult<Option<Vec<u16>>> {
    with_page(agent, |agent, page| {
        let map = page.custom_elements.form_data;
        let Some(data) = PrivateArray::from_value(agent.weak_map_get(map, target)) else {
            return Ok(None);
        };
        let value = data.get(agent, SUBMISSION)?;
        if value == Value::NULL {
            return Ok(None);
        }
        let value = creation::string(agent, value)?;
        Ok(Some(value.as_units().to_vec()))
    })
}

pub(in crate::page) fn validity_bits(agent: &mut Agent, target: Value) -> JsResult<u16> {
    with_page(agent, |agent, page| {
        let map = page.custom_elements.form_data;
        let Some(data) = PrivateArray::from_value(agent.weak_map_get(map, target)) else {
            return Ok(0);
        };
        // The record only stores exact 10-bit masks from setValidity.
        Ok(data
            .get(agent, VALIDITY)?
            .as_number()
            .expect("validity bits") as u16)
    })
}

fn form(agent: &mut Agent, internals: Value, _: &[Value]) -> JsResult<Value> {
    let target = form_target(agent, internals)?;
    super::super::events::custom_form_status(agent, target).map(|(form, _)| form)
}

fn will_validate(agent: &mut Agent, internals: Value, _: &[Value]) -> JsResult<Value> {
    let target = form_target(agent, internals)?;
    super::super::events::form_will_validate(agent, target)
}

fn validity(agent: &mut Agent, internals: Value, _: &[Value]) -> JsResult<Value> {
    let target = form_target(agent, internals)?;
    super::super::events::validity_state(agent, target)
}

fn check_validity(agent: &mut Agent, internals: Value, _: &[Value]) -> JsResult<Value> {
    let target = form_target(agent, internals)?;
    super::super::events::check_control(agent, target).map(Value::boolean)
}

fn validation_message(agent: &mut Agent, internals: Value, _: &[Value]) -> JsResult<Value> {
    let target = form_target(agent, internals)?;
    if super::super::events::form_will_validate(agent, target)? == Value::TRUE {
        let record = data(agent, target)?;
        let message = record.get(agent, MESSAGE)?;
        if message != Value::UNDEFINED {
            return Ok(message);
        }
    }
    agent.string("")
}

fn labels(agent: &mut Agent, internals: Value, _: &[Value]) -> JsResult<Value> {
    let target = form_target(agent, internals)?;
    let data = data(agent, target)?;
    let cached = data.get(agent, LABELS)?;
    if cached != Value::UNDEFINED {
        return Ok(cached);
    }
    let labels = super::super::events::custom_labels(agent, target)?;
    data.set(agent, LABELS, labels)?;
    Ok(labels)
}

fn convert_submission(agent: &mut Agent, value: Value) -> JsResult<Value> {
    if value == Value::NULL {
        return Ok(value);
    }
    let string = creation::string(agent, value)?;
    crate::webidl::from_dom_string(agent, &string)
}

fn set_form_value(agent: &mut Agent, internals: Value, args: &[Value]) -> JsResult<Value> {
    let target = form_target(agent, internals)?;
    let argument = required_arg(agent, args, 0)?;
    let value = convert_submission(agent, argument)?;
    with_root(agent, value, |agent| {
        let state = match args.get(1).copied().filter(|&arg| arg != Value::UNDEFINED) {
            Some(state) => convert_submission(agent, state)?,
            None => value,
        };
        with_root(agent, state, |agent| {
            let data = data(agent, target)?;
            data.set(agent, SUBMISSION, value)?;
            data.set(agent, STATE, state)?;
            Ok(Value::UNDEFINED)
        })
    })
}

fn set_validity(agent: &mut Agent, internals: Value, args: &[Value]) -> JsResult<Value> {
    let target = form_target(agent, internals)?;
    let flags = required_arg(agent, args, 0)?;
    let mut bits = 0u16;
    if let Some(object) = flags.as_object() {
        for (index, name) in FLAGS.iter().enumerate() {
            let key = agent.heap.key(name)?;
            let flag = agent.get(object, key)?;
            if agent.to_boolean(flag) {
                bits |= 1 << index;
            }
        }
    } else if !flags.is_nullish() {
        return agent.type_error("expected ValidityStateFlags");
    }
    let message = match args
        .get(1)
        .copied()
        .filter(|&value| value != Value::UNDEFINED)
    {
        Some(value) => creation::string(agent, value)?,
        None => rustic_dom::DomString::default(),
    };
    if bits != 0 && message.as_units().is_empty() {
        return agent.type_error("invalid states require a validation message");
    }
    let anchor = args
        .get(2)
        .copied()
        .filter(|value| *value != Value::UNDEFINED)
        .unwrap_or(Value::NULL);
    if anchor != Value::NULL {
        let id = super::node_of(agent, target)?;
        let is_descendant = read_node(agent, anchor, |agent, _, document, _, node| {
            if !matches!(node.kind(), NodeKind::Element(_)) {
                return agent.type_error("expected an Element validation anchor");
            }
            let mut parent = node.parent().or_else(|| node.shadow_host());
            while let Some(ancestor) = parent {
                agent.check_native_work()?;
                if ancestor == id {
                    return Ok(true);
                }
                let node = document.node(ancestor).expect("live anchor ancestor");
                parent = node.parent().or_else(|| node.shadow_host());
            }
            Ok(false)
        })?;
        if !is_descendant {
            return Err(creation::exception(
                agent,
                "NotFoundError",
                "anchor is outside the custom element",
            ));
        }
    }
    let message = if bits == 0 {
        Value::UNDEFINED
    } else {
        crate::webidl::from_dom_string(agent, &message)?
    };
    with_root(agent, message, |agent| {
        let data = data(agent, target)?;
        data.set(agent, VALIDITY, Value::number(f64::from(bits)))?;
        data.set(agent, MESSAGE, message)?;
        data.set(agent, ANCHOR, anchor)?;
        Ok(Value::UNDEFINED)
    })
}
