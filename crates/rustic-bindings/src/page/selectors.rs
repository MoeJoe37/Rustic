//! DOM selectors backed by the CSS parser and style matcher.

use rustic_css::{CssError, CssLimits, CssParser, NamespaceMap};
use rustic_dom::{DomString, NodeKind};
use rustic_js::runtime::{Abrupt, Agent, JsResult, NativeFn, ObjectId, Value};
use rustic_style::{ScopedSelector, SelectorQueryError, SelectorWork};

use crate::dom_exception;
use crate::webidl::to_usv_string;

use super::{collections, read_node, require_receiver, required_arg, with_page, wrap};

const UNMETERED_HOST_ALLOWANCE: u64 = 1_000_000;

pub(super) fn install_parent_node(agent: &mut Agent, prototype: ObjectId) -> JsResult<()> {
    install_methods(
        agent,
        prototype,
        &[
            ("querySelector", query_selector),
            ("querySelectorAll", query_selector_all),
        ],
    )
}

pub(super) fn install_non_element_parent(agent: &mut Agent, prototype: ObjectId) -> JsResult<()> {
    install_methods(agent, prototype, &[("getElementById", get_element_by_id)])
}

pub(super) fn install_element(agent: &mut Agent, prototype: ObjectId) -> JsResult<()> {
    install_methods(
        agent,
        prototype,
        &[("matches", matches), ("closest", closest)],
    )
}

fn install_methods(
    agent: &mut Agent,
    prototype: ObjectId,
    methods: &[(&str, NativeFn)],
) -> JsResult<()> {
    for &(name, behavior) in methods {
        let key = agent.heap.key(name)?;
        agent.define_method(prototype, key, behavior, name, 1)?;
    }
    Ok(())
}

fn is_parent(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Element(_) | NodeKind::Document | NodeKind::DocumentFragment
    )
}

fn is_element(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Element(_))
}

fn exception(agent: &mut Agent, name: &str, message: &str) -> Abrupt {
    match dom_exception::new(agent, name, message) {
        Ok(value) => Abrupt::Throw(value),
        Err(error) => error,
    }
}

fn query<T>(
    agent: &mut Agent,
    receiver: Value,
    args: &[Value],
    brand: fn(&NodeKind) -> bool,
    run: impl FnOnce(&ScopedSelector<'_>) -> Result<T, SelectorQueryError>,
) -> JsResult<T> {
    require_receiver(agent, receiver, brand, "invalid selector receiver")?;
    let input = required_arg(agent, args, 0)?;
    let input = to_usv_string(agent, input)?;
    let token = agent.cancellation();
    let limits = CssLimits {
        max_diagnostics: 0,
        ..CssLimits::default()
    };
    if let Some(left) = agent.step_budget() {
        let cost = (input.len().min(limits.max_input_bytes) as u64).div_ceil(64);
        let Some(remaining) = left.checked_sub(cost) else {
            agent.set_step_budget(Some(0));
            return Err(Abrupt::Cancelled);
        };
        agent.set_step_budget(Some(remaining));
    }
    let parser = CssParser::new(limits, token.clone().unwrap_or_default())
        .expect("valid default CSS limits");
    let parsed =
        parser
            .selector_list(&input, &NamespaceMap::new())
            .map_err(|error| match error {
                CssError::Cancelled => Abrupt::Cancelled,
                CssError::AllocationFailed => Abrupt::OutOfMemory,
                _ => exception(
                    agent,
                    "QuotaExceededError",
                    "selector parsing limit exceeded",
                ),
            })?;
    let list = parsed
        .value
        .map_err(|_| exception(agent, "SyntaxError", "invalid selector"))?;
    let quirks = with_page(agent, |_, page| Ok(page.tree.quirks_mode()))?;
    let limit = agent.step_budget();
    let work = SelectorWork::new(limit.unwrap_or(UNMETERED_HOST_ALLOWANCE), token.as_ref());
    let result = read_node(agent, receiver, |_, _, document, id, _| {
        Ok(run(&ScopedSelector::new(
            document, id, &list, quirks, &work,
        )))
    })?;
    if limit.is_some() {
        agent.set_step_budget(Some(work.remaining()));
    }
    result.map_err(|error| match error {
        SelectorQueryError::Cancelled => Abrupt::Cancelled,
        SelectorQueryError::WorkLimit if limit.is_some() => Abrupt::Cancelled,
        SelectorQueryError::WorkLimit => {
            exception(agent, "QuotaExceededError", "selector work limit exceeded")
        }
        SelectorQueryError::Dom(error) => dom_exception::from_dom_error(agent, error),
    })
}

fn wrap_optional(
    agent: &mut Agent,
    receiver: Value,
    id: Option<rustic_dom::NodeId>,
) -> JsResult<Value> {
    match id {
        Some(id) => read_node(agent, receiver, |agent, wrappers, document, _, _| {
            wrap(wrappers, agent, document, id)
        }),
        None => Ok(Value::NULL),
    }
}

fn query_selector(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let result = query(agent, receiver, args, is_parent, |selector| {
        selector.first()
    })?;
    wrap_optional(agent, receiver, result)
}

fn query_selector_all(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let result = query(agent, receiver, args, is_parent, |selector| selector.all())?;
    read_node(agent, receiver, |agent, wrappers, document, _, _| {
        collections::snapshot(agent, wrappers, document, result)
    })
}

fn matches(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    query(agent, receiver, args, is_element, |selector| {
        selector.matches()
    })
    .map(Value::boolean)
}

fn closest(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let result = query(agent, receiver, args, is_element, |selector| {
        selector.closest()
    })?;
    wrap_optional(agent, receiver, result)
}

fn get_element_by_id(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    require_receiver(
        agent,
        receiver,
        |kind| matches!(kind, NodeKind::Document | NodeKind::DocumentFragment),
        "expected a Document or DocumentFragment receiver",
    )?;
    let input = required_arg(agent, args, 0)?;
    let sought: DomString = super::creation::string(agent, input)?;
    if sought.as_units().is_empty() {
        return Ok(Value::NULL);
    }
    read_node(agent, receiver, |agent, wrappers, document, id, _| {
        let walk = document
            .walk(id)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        for (id, _) in walk.skip(1) {
            agent.check_native_work()?;
            let node = document
                .node(id)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
            if let NodeKind::Element(element) = node.kind() {
                for attr in element.attributes() {
                    agent.check_native_work()?;
                    let name = attr.name();
                    if name.namespace().is_none() && name.local() == "id" && attr.value() == &sought
                    {
                        return wrap(wrappers, agent, document, id);
                    }
                }
            }
        }
        Ok(Value::NULL)
    })
}
