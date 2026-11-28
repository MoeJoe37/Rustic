//! Scoped registry identity and tree assignment.

use rustic_dom::{Document, NodeId, NodeKind};
use rustic_js::runtime::{Abrupt, Agent, JsResult, Value};

use crate::heap::DomHeap;

use super::super::read_node;
use super::{
    Definitions, INTERFACE, Page, PrivateArray, Rc, receiver, required_arg, upgrade_tree,
    with_page, with_root, wrap_node,
};

pub(super) fn construct(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("CustomElementRegistry requires new");
    };
    let proto = agent.host_prototype_from_new_target(INTERFACE, new_target)?;
    let object = agent.new_object_with(Some(proto))?;
    let value = Value::object(object);
    with_root(agent, value, |agent| {
        let roots = PrivateArray::new(agent, &[])?;
        with_page(agent, |agent, page| {
            let registry = &page.custom_elements;
            let mut scoped = registry.scoped.borrow_mut();
            scoped.retain(|&key, _| agent.weak_map_has(registry.roots, Value::object(key)));
            scoped.try_reserve(1).map_err(|_| Abrupt::OutOfMemory)?;
            agent.weak_map_set(registry.roots, value, roots.value())?;
            scoped.insert(object, Rc::new(Definitions::new(roots)));
            Ok(value)
        })
    })
}

pub(in crate::page) fn convert_registry(
    agent: &mut Agent,
    value: Value,
) -> JsResult<Option<Value>> {
    if value == Value::UNDEFINED {
        return Ok(None);
    }
    if value != Value::NULL {
        receiver(agent, value)?;
    }
    Ok(Some(value))
}

pub(in crate::page) fn set_registry(agent: &mut Agent, node: Value, scope: Value) -> JsResult<()> {
    with_page(agent, |agent, page| {
        agent.weak_map_set(page.custom_elements.assignments, node, scope)
    })
}

pub(in crate::page) fn registry_of(agent: &mut Agent, node: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, node, |agent, wrappers, doc, id, node| {
        if !matches!(node.kind(), NodeKind::Element(_) | NodeKind::Document)
            && node.shadow_host().is_none()
        {
            return agent.type_error("expected a Document, Element or ShadowRoot");
        }
        with_page(agent, |agent, page| {
            scope_in_tree(agent, page, wrappers, doc, id)
        })
    })
}

fn explicit(agent: &Agent, page: &Page, wrappers: &DomHeap, id: NodeId) -> Option<Value> {
    let wrapper = wrappers.existing_wrapper(id)?;
    let value = agent.weak_map_get(page.custom_elements.assignments, Value::object(wrapper));
    (value != Value::UNDEFINED).then_some(value)
}

fn document_registry(agent: &Agent, page: &Page, wrappers: &DomHeap, doc: &Document) -> Value {
    explicit(agent, page, wrappers, doc.root()).unwrap_or(if doc.id() == page.main {
        page.custom_elements.object
    } else {
        Value::NULL
    })
}

fn scope_in_tree(
    agent: &mut Agent,
    page: &Page,
    wrappers: &DomHeap,
    doc: &Document,
    start: NodeId,
) -> JsResult<Value> {
    let mut current = Some(start);
    while let Some(id) = current {
        agent.check_native_work()?;
        if let Some(scope) = explicit(agent, page, wrappers, id) {
            return Ok(scope);
        }
        let node = doc.node(id).expect("live registry ancestor");
        if matches!(node.kind(), NodeKind::Element(element) if element.keeps_registry_null())
            || node.shadow_host().is_some_and(|host| {
                matches!(doc.node(host).expect("live host").kind(), NodeKind::Element(element)
                    if element.shadow_root_options().is_some_and(|options| options.keep_registry_null))
            })
        {
            return Ok(Value::NULL);
        }
        current = node.parent();
    }
    Ok(document_registry(agent, page, wrappers, doc))
}

pub(super) fn definition_added(
    agent: &mut Agent,
    page: &Page,
    scope: Value,
    name: &str,
) -> JsResult<()> {
    let mut documents = Vec::new();
    {
        let additional = page.additional.borrow();
        documents
            .try_reserve_exact(additional.len() + 1)
            .map_err(|_| Abrupt::OutOfMemory)?;
        documents.push(page.main);
        documents.extend(additional.keys().copied());
    }
    documents.sort_unstable();
    for document in documents {
        if let Some(root) = page.read(agent, document, |_, doc| Ok(Some(doc.root())))? {
            upgrade_tree(agent, root, Some(name), scope)?;
        }
    }
    Ok(())
}

pub(super) fn initialize(agent: &mut Agent, scope: Value, args: &[Value]) -> JsResult<Value> {
    receiver(agent, scope)?;
    let input = required_arg(agent, args, 0)?;
    let (needs_assignment, nodes) = read_node(agent, input, |agent, wrappers, doc, root, node| {
        let is_document = matches!(node.kind(), NodeKind::Document);
        let can_assign_root = is_document || node.shadow_host().is_some();
        with_page(agent, |agent, page| {
            if scope == page.custom_elements.object
                && (is_document || document_registry(agent, page, wrappers, doc) != scope)
            {
                return Err(super::unsupported(
                    agent,
                    "registry cannot initialize this root",
                ));
            }
            let mut nodes = Vec::new();
            // Collect before changing the root so null descendants keep their
            // identification for assignment and upgrades skip nested shadow trees.
            for (id, _) in doc
                .walk(root)
                .map_err(|error| crate::dom_exception::from_dom_error(agent, error))?
            {
                agent.check_native_work()?;
                if matches!(
                    doc.node(id).expect("walked node").kind(),
                    NodeKind::Element(_)
                ) {
                    nodes.try_reserve(1).map_err(|_| Abrupt::OutOfMemory)?;
                    let previous = scope_in_tree(agent, page, wrappers, doc, id)?;
                    nodes.push((id, previous));
                }
            }
            Ok((
                can_assign_root && scope_in_tree(agent, page, wrappers, doc, root)? == Value::NULL,
                nodes,
            ))
        })
    })?;
    if needs_assignment {
        set_registry(agent, input, scope)?;
    }
    for &(id, previous) in &nodes {
        if previous == Value::NULL {
            let wrapper = wrap_node(agent, id)?;
            set_registry(agent, wrapper, scope)?;
        }
    }
    with_page(agent, |agent, page| {
        for (id, previous) in nodes {
            if (previous == Value::NULL || previous == scope) && page.custom_elements.active() {
                super::try_upgrade(agent, page, id, None, Some(scope))?;
            }
        }
        Ok(Value::UNDEFINED)
    })
}

pub(in crate::page) fn check_global(
    agent: &mut Agent,
    document: Value,
    scope: Value,
) -> JsResult<()> {
    with_page(agent, |agent, page| {
        if scope == page.custom_elements.object && registry_of(agent, document, &[])? != scope {
            return Err(super::unsupported(
                agent,
                "foreign global custom element registry",
            ));
        }
        Ok(())
    })
}

pub(in crate::page) fn owner_registry(agent: &mut Agent, id: NodeId) -> JsResult<Value> {
    let root = super::read_doc(agent, id, Document::root)?;
    let document = wrap_node(agent, root)?;
    registry_of(agent, document, &[])
}

pub(in crate::page) fn copy_scopes(
    agent: &mut Agent,
    wrappers: &mut DomHeap,
    (source, original): (&Document, NodeId),
    (target, copy): (&Document, NodeId),
    deep: bool,
    fallback: Value,
) -> JsResult<()> {
    struct Pair {
        original: NodeId,
        copy: NodeId,
        deep: bool,
        inherited: Value,
        copy_inherited: Value,
        fallback: Value,
    }
    with_page(agent, |agent, page| {
        let registry = &page.custom_elements;
        let global = if target.id() == page.main {
            registry.object
        } else {
            Value::NULL
        };
        let mut pending = vec![Pair {
            original,
            copy,
            deep,
            fallback,
            inherited: scope_in_tree(agent, page, wrappers, source, original)?,
            copy_inherited: document_registry(agent, page, wrappers, target),
        }];
        while let Some(mut pair) = pending.pop() {
            agent.check_native_work()?;
            let node = source.node(pair.original).expect("live original");
            let copy = target.node(pair.copy).expect("live copy");
            let assigned = explicit(agent, page, wrappers, pair.original);
            let mut scope = assigned.unwrap_or(pair.inherited);
            let mut copied = pair.copy_inherited;
            match (node.kind(), copy.kind()) {
                (NodeKind::Document, NodeKind::Document)
                    if scope != registry.object && scope != Value::NULL =>
                {
                    assign(agent, page, wrappers, target, pair.copy, scope)?;
                    copied = scope;
                }
                (NodeKind::Element(element), NodeKind::Element(clone)) => {
                    if assigned.is_none() && element.keeps_registry_null() {
                        scope = Value::NULL;
                    }
                    let keep_null = element.keeps_registry_null() || assigned == Some(Value::NULL);
                    let remapped = if scope == registry.object {
                        global
                    } else if scope == Value::NULL {
                        if keep_null {
                            pair.fallback = Value::NULL;
                        }
                        pair.fallback
                    } else {
                        scope
                    };
                    if remapped != copied
                        || (remapped != registry.object && remapped != Value::NULL)
                        || (remapped == Value::NULL && keep_null)
                    {
                        assign(agent, page, wrappers, target, pair.copy, remapped)?;
                    }
                    copied = remapped;
                    if let Some((original, copy)) = element.shadow_root().zip(clone.shadow_root()) {
                        let inherited = scope_in_tree(agent, page, wrappers, source, original)?;
                        let copy_inherited = if inherited == registry.object {
                            global
                        } else {
                            inherited
                        };
                        assign(agent, page, wrappers, target, copy, copy_inherited)?;
                        pending.try_reserve(1).map_err(|_| Abrupt::OutOfMemory)?;
                        pending.push(Pair {
                            original,
                            copy,
                            deep: true,
                            inherited,
                            copy_inherited,
                            fallback: Value::NULL,
                        });
                    }
                    if pair.deep {
                        if let Some((original, copy)) =
                            element.template_contents().zip(clone.template_contents())
                        {
                            let inherited = scope_in_tree(agent, page, wrappers, source, original)?;
                            pending.try_reserve(1).map_err(|_| Abrupt::OutOfMemory)?;
                            pending.push(Pair {
                                original,
                                copy,
                                deep: true,
                                inherited,
                                copy_inherited: global,
                                fallback: Value::NULL,
                            });
                        }
                    }
                }
                _ => {}
            }
            if pair.deep {
                let originals = source.children(pair.original).expect("live original");
                let copies = target.children(pair.copy).expect("live copy");
                for (original, copy) in originals.zip(copies) {
                    pending.try_reserve(1).map_err(|_| Abrupt::OutOfMemory)?;
                    pending.push(Pair {
                        original,
                        copy,
                        deep: true,
                        inherited: scope,
                        copy_inherited: copied,
                        fallback: pair.fallback,
                    });
                }
            }
        }
        Ok(())
    })
}

fn assign(
    agent: &mut Agent,
    page: &Page,
    wrappers: &mut DomHeap,
    document: &Document,
    node: NodeId,
    scope: Value,
) -> JsResult<()> {
    let value = super::super::wrap(wrappers, agent, document, node)?;
    agent.weak_map_set(page.custom_elements.assignments, value, scope)
}

pub(in crate::page) fn upgrade_copy(agent: &mut Agent, copy: Value) -> JsResult<Value> {
    with_root(agent, copy, |agent| {
        with_page(agent, |agent, page| {
            if page.custom_elements.active() {
                let root = super::node_of(agent, copy)?;
                for node in super::candidates(agent, root)? {
                    super::try_upgrade(agent, page, node, None, None)?;
                }
            }
            Ok(copy)
        })
    })
}
