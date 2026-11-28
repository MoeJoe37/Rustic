//! Frozen propagation paths with shadow retargeting and closed-tree visibility.

use rustic_dom::{Document, NodeId, ShadowRootMode};
use rustic_js::runtime::{Agent, JsResult, OutOfMemory, Value};

use crate::dom_exception;
use crate::heap::{DomHeap, WrapperKind};

use super::super::{read_node, with_page, wrap};
use super::{PrivateArray, Target, with_root};

const AT_TARGET: u8 = 1;
const CLOSED_ROOT: u8 = 2;
const CLOSED_SLOT: u8 = 4;

#[derive(Clone, Copy)]
pub(super) struct Path(PrivateArray);

#[derive(Clone, Copy)]
pub(super) struct Step {
    pub target: Value,
    pub adjusted: Value,
    pub related: Value,
    flags: u8,
}

impl Step {
    pub fn at_target(self) -> bool {
        self.flags & AT_TARGET != 0
    }
}

pub(super) fn tree_root(
    agent: &mut Agent,
    document: &Document,
    mut node: NodeId,
) -> JsResult<NodeId> {
    loop {
        agent.check_native_work()?;
        let Some(parent) = document.node(node).expect("live event node").parent() else {
            return Ok(node);
        };
        node = parent;
    }
}

fn shadow_host(document: &Document, node: NodeId) -> Option<NodeId> {
    document.node(node).expect("live event node").shadow_host()
}

fn closed(document: &Document, root: NodeId) -> bool {
    let Some(host) = shadow_host(document, root) else {
        return false;
    };
    let rustic_dom::NodeKind::Element(element) = document.node(host).expect("live host").kind()
    else {
        unreachable!("shadow host is an element")
    };
    element.shadow_root_mode() == Some(ShadowRootMode::Closed)
}

/// DOM Standard retarget algorithm. A null reference represents Window.
pub(super) fn retarget(
    agent: &mut Agent,
    document: &Document,
    mut target: NodeId,
    mut root: NodeId,
    against: Option<NodeId>,
) -> JsResult<(NodeId, NodeId)> {
    while let Some(host) = shadow_host(document, root) {
        let mut reference = against;
        while let Some(node) = reference {
            let ancestor = tree_root(agent, document, node)?;
            if ancestor == root {
                return Ok((target, root));
            }
            reference = shadow_host(document, ancestor);
        }
        target = host;
        root = tree_root(agent, document, host)?;
    }
    Ok((target, root))
}

fn append(
    agent: &mut Agent,
    targets: &mut Vec<Value>,
    fields: &mut Vec<Value>,
    step: Step,
) -> JsResult<()> {
    agent.check_native_work()?;
    targets.try_reserve(1).map_err(|_| OutOfMemory)?;
    fields.try_reserve(3).map_err(|_| OutOfMemory)?;
    targets.push(step.target);
    fields.extend([
        step.adjusted,
        step.related,
        Value::number(f64::from(step.flags)),
    ]);
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) struct Init {
    pub exposed: Value,
    pub related: Value,
    pub composed: bool,
    pub load: bool,
}

impl Path {
    pub fn build(agent: &mut Agent, target: Target, init: Init) -> JsResult<Self> {
        if !target.node {
            return Self::new(
                agent,
                &[target.value],
                &[
                    init.exposed,
                    init.related,
                    Value::number(f64::from(AT_TARGET)),
                ],
                false,
            );
        }
        with_page(agent, |agent, page| {
            read_node(agent, target.value, |agent, wrappers, doc, start, _| {
                let local_related = init
                    .related
                    .as_object()
                    .and_then(|object| wrappers.identity(agent, object, WrapperKind::Node))
                    .filter(|node| node.document() == doc.id() && doc.node(*node).is_ok());
                let related_tree = local_related
                    .map(|id| tree_root(agent, doc, id))
                    .transpose()?;
                let original_related = local_related.zip(related_tree);
                let mut slots = None;
                let mut targets = Vec::new();
                let mut fields = Vec::new();
                let start_root = tree_root(agent, doc, start)?;
                let mut adjusted_node = start;
                let mut adjusted_root = start_root;
                let mut clear_targets = false;
                let mut current = Some(start);
                let mut closed_slot = false;
                while let Some(node) = current {
                    let mut flags = u8::from(node == start) * AT_TARGET;
                    let against = Some(node);
                    let (adjusted, root) =
                        retarget(agent, doc, adjusted_node, adjusted_root, against)?;
                    if adjusted != adjusted_node {
                        flags |= AT_TARGET;
                        adjusted_node = adjusted;
                        adjusted_root = root;
                    }
                    if closed(doc, node) {
                        flags |= CLOSED_ROOT;
                    }
                    if closed_slot {
                        flags |= CLOSED_SLOT;
                    }
                    let (related, related_root) = if let Some((related, root)) = original_related {
                        let (related, root) = retarget(agent, doc, related, root, against)?;
                        if adjusted == related && (node != start || local_related != Some(start)) {
                            break;
                        }
                        (
                            wrapper(agent, wrappers, doc, related, start, target.value)?,
                            Some(root),
                        )
                    } else {
                        (init.related, None)
                    };
                    let exposed =
                        wrapper(agent, wrappers, doc, adjusted_node, start, init.exposed)?;
                    let target = wrapper(agent, wrappers, doc, node, start, target.value)?;
                    append(
                        agent,
                        &mut targets,
                        &mut fields,
                        Step {
                            target,
                            adjusted: exposed,
                            related,
                            flags,
                        },
                    )?;
                    clear_targets = shadow_host(doc, adjusted_root).is_some()
                        || related_root.is_some_and(|root| shadow_host(doc, root).is_some());
                    if node == doc.root() && doc.id() == page.main && !init.load {
                        append(
                            agent,
                            &mut targets,
                            &mut fields,
                            Step {
                                target: page.window,
                                adjusted: exposed,
                                related,
                                flags: 0,
                            },
                        )?;
                        break;
                    }
                    let at = doc.node(node).expect("live event path node");
                    closed_slot = false;
                    let assigned = if at.parent().is_some_and(|parent| {
                        matches!(doc.node(parent).expect("live parent").kind(), rustic_dom::NodeKind::Element(host)
                            if host.shadow_root().is_some())
                    }) {
                        if slots.is_none() {
                            slots = Some(doc.slot_map().map_err(|error| dom_exception::from_dom_error(agent, error))?);
                        }
                        slots.as_ref().and_then(|slots| slots.assigned_slot(node))
                    } else {
                        None
                    };
                    if let Some(slot) = assigned {
                        closed_slot = closed(doc, tree_root(agent, doc, slot)?);
                        current = Some(slot);
                    } else if let Some(host) = at.shadow_host() {
                        current = (init.composed || node != start_root).then_some(host);
                    } else {
                        current = at.parent();
                    }
                }
                Self::new(agent, &targets, &fields, clear_targets)
            })
        })
    }

    fn new(agent: &mut Agent, targets: &[Value], fields: &[Value], clear: bool) -> JsResult<Self> {
        let targets = PrivateArray::new(agent, targets)?;
        with_root(agent, targets.value(), |agent| {
            let fields = PrivateArray::new(agent, fields)?;
            let path = PrivateArray::new(
                agent,
                &[targets.value(), fields.value(), Value::boolean(clear)],
            )?;
            Ok(Self(path))
        })
    }

    pub fn from_value(value: Value) -> Option<Self> {
        PrivateArray::from_value(value).map(Self)
    }

    pub fn value(self) -> Value {
        self.0.value()
    }

    pub fn targets(self, agent: &mut Agent) -> JsResult<PrivateArray> {
        Ok(PrivateArray::from_value(self.0.get(agent, 0)?).expect("path targets"))
    }

    pub fn len(self, agent: &mut Agent) -> JsResult<u32> {
        self.targets(agent)?.len(agent)
    }

    pub fn step(self, agent: &mut Agent, index: u32) -> JsResult<Step> {
        let target = self.targets(agent)?.get(agent, index)?;
        let data = PrivateArray::from_value(self.0.get(agent, 1)?).expect("path fields");
        // A path has exactly three private array fields for each target.
        let field = index * 3;
        let flags = data
            .get(agent, field + 2)?
            .as_number()
            .expect("event path flags") as u8;
        Ok(Step {
            target,
            adjusted: data.get(agent, field)?,
            related: data.get(agent, field + 1)?,
            flags,
        })
    }

    pub fn final_targets(self, agent: &mut Agent, related: Value) -> JsResult<(Value, Value)> {
        if self.0.get(agent, 2)? == Value::TRUE {
            return Ok((Value::NULL, Value::NULL));
        }
        let Some(last) = self.len(agent)?.checked_sub(1) else {
            return Ok((Value::NULL, related));
        };
        let step = self.step(agent, last)?;
        Ok((step.adjusted, step.related))
    }

    pub fn visible(self, agent: &mut Agent, current: Value) -> JsResult<Vec<Value>> {
        let len = self.len(agent)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(len as usize)
            .map_err(|_| OutOfMemory)?;

        // DOM composedPath starts from the listener's position and uses a
        // separate hidden subtree level in each direction.
        let mut hidden = 0;
        let mut listener = None;
        for index in (0..len).rev() {
            agent.check_native_work()?;
            let step = self.step(agent, index)?;
            hidden += i32::from(step.flags & CLOSED_ROOT != 0);
            if step.target == current {
                listener = Some(index);
                break;
            }
            hidden -= i32::from(step.flags & CLOSED_SLOT != 0);
        }
        let Some(listener) = listener else {
            return Ok(values);
        };
        let mut level = hidden;
        let mut threshold = hidden;
        for index in (0..listener).rev() {
            agent.check_native_work()?;
            let step = self.step(agent, index)?;
            level += i32::from(step.flags & CLOSED_ROOT != 0);
            if level <= threshold {
                values.push(step.target);
            }
            if step.flags & CLOSED_SLOT != 0 {
                level -= 1;
                threshold = threshold.min(level);
            }
        }
        values.reverse();
        values.push(current);
        level = hidden;
        threshold = hidden;
        for index in listener + 1..len {
            agent.check_native_work()?;
            let step = self.step(agent, index)?;
            level += i32::from(step.flags & CLOSED_SLOT != 0);
            if level <= threshold {
                values.push(step.target);
            }
            if step.flags & CLOSED_ROOT != 0 {
                level -= 1;
                threshold = threshold.min(level);
            }
        }
        Ok(values)
    }
}

fn wrapper(
    agent: &mut Agent,
    wrappers: &mut DomHeap,
    document: &Document,
    node: NodeId,
    start: NodeId,
    original: Value,
) -> JsResult<Value> {
    if node == start {
        return Ok(original);
    }
    wrap(wrappers, agent, document, node)
}
