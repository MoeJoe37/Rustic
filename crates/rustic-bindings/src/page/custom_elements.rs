//! The Window custom-element registry and reactions for autonomous elements.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rustic_dom::{
    DomString, HTML_NAMESPACE, MutationEntry, NodeId, NodeKind, QualifiedName,
    is_custom_element_name,
};
use rustic_js::runtime::{Abrupt, Agent, HostRoot, JsResult, ObjectId, Value};

use super::events::{PrivateArray, define_method, with_root};
use super::traversal::{node_of, read_doc, wrap_node};
use super::{
    Page, create_node, creation, illegal_constructor, install_interface, prototype, required_arg,
    with_page,
};

const INTERFACE: &str = "CustomElementRegistry";

#[derive(Debug)]
struct Definition {
    name: String,
    constructor: Value,
    connected: Value,
    disconnected: Value,
    adopted: Value,
    attribute_changed: Value,
    observed: Vec<DomString>,
}

#[derive(Debug)]
struct Construction {
    constructor: Value,
    element: Option<Value>,
}

#[derive(Debug)]
pub(super) struct Registry {
    object: Value,
    definitions: RefCell<HashMap<String, Rc<Definition>>>,
    // Definition functions and pending promises are owned by this registry.
    roots: PrivateArray,
    waiting: RefCell<HashMap<String, (ObjectId, HostRoot)>>,
    states: ObjectId,
    owners: ObjectId,
    stack: RefCell<Vec<Construction>>,
    defining: Cell<bool>,
}

impl Registry {
    pub(super) fn active(&self) -> bool {
        !self.definitions.borrow().is_empty()
    }
}

pub(super) fn install(agent: &mut Agent) -> JsResult<Registry> {
    install_interface(
        agent,
        INTERFACE,
        None,
        illegal_constructor,
        0,
        &[],
        |agent, proto| {
            define_method(agent, proto, "define", define, 2)?;
            define_method(agent, proto, "get", get, 1)?;
            define_method(agent, proto, "getName", get_name, 1)?;
            define_method(agent, proto, "whenDefined", when_defined, 1)?;
            define_method(agent, proto, "upgrade", upgrade, 1)
        },
    )?;
    let proto = prototype(agent, INTERFACE)?;
    let object = Value::object(agent.new_object_with(Some(proto))?);
    let roots = PrivateArray::new(agent, &[])?;
    let states = agent.new_weak_map()?;
    let owners = agent.new_weak_map()?;
    let memory = PrivateArray::new(
        agent,
        &[
            object,
            roots.value(),
            Value::object(states),
            Value::object(owners),
        ],
    )?;
    agent.root(memory.value())?;
    let global = agent.intrinsics.global;
    super::elements::accessor(agent, global, "customElements", registry, None)?;
    Ok(Registry {
        object,
        roots,
        states,
        owners,
        definitions: RefCell::new(HashMap::new()),
        waiting: RefCell::new(HashMap::new()),
        stack: RefCell::new(Vec::new()),
        defining: Cell::new(false),
    })
}

fn registry(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    with_page(agent, |_, page| Ok(page.custom_elements.object))
}

fn receiver(agent: &mut Agent, value: Value) -> JsResult<()> {
    if registry(agent, Value::UNDEFINED, &[])? != value {
        return agent.type_error("expected CustomElementRegistry");
    }
    Ok(())
}

fn name(agent: &mut Agent, args: &[Value]) -> JsResult<Option<String>> {
    let input = required_arg(agent, args, 0)?;
    let input = creation::string(agent, input)?;
    creation::scalar_name(&input)
}

fn syntax_error(agent: &mut Agent) -> Abrupt {
    creation::exception(agent, "SyntaxError", "invalid custom element name")
}

fn unsupported(agent: &mut Agent, message: &str) -> Abrupt {
    creation::exception(agent, "NotSupportedError", message)
}

fn callback(agent: &mut Agent, proto: ObjectId, name: &str) -> JsResult<Value> {
    let key = agent.heap.key(name)?;
    let value = agent.get(proto, key)?;
    if value != Value::UNDEFINED && !agent.is_callable(value) {
        return agent.type_error("custom element callback is not callable");
    }
    Ok(value)
}

fn define(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    receiver(agent, this)?;
    let local = name(agent, args)?
        .filter(|name| is_custom_element_name(name))
        .ok_or_else(|| syntax_error(agent))?;
    let constructor = required_arg(agent, args, 1)?;
    if !agent.is_constructor_value(constructor) {
        return agent.type_error("custom element definition requires a constructor");
    }
    if let Some(options) = args.get(2).and_then(|value| value.as_object()) {
        let key = agent.heap.key("extends")?;
        if agent.get(options, key)? != Value::UNDEFINED {
            return Err(unsupported(
                agent,
                "customized built-in elements are unavailable",
            ));
        }
    }
    with_page(agent, |agent, page| {
        let registry = &page.custom_elements;
        {
            let definitions = registry.definitions.borrow();
            if definitions.contains_key(&local)
                || definitions
                    .values()
                    .any(|def| def.constructor == constructor)
            {
                return Err(unsupported(agent, "duplicate custom element definition"));
            }
        }
        if registry.defining.replace(true) {
            return Err(unsupported(
                agent,
                "a custom element definition is already being read",
            ));
        }
        let result = read_definition(agent, local.clone(), constructor);
        registry.defining.set(false);
        let definition = Rc::new(result?);
        super::mutation_observer::enable_journals(agent, page)?;
        for value in [
            constructor,
            definition.connected,
            definition.disconnected,
            definition.adopted,
            definition.attribute_changed,
        ] {
            let index = registry.roots.len(agent)?;
            registry.roots.set(agent, index, value)?;
        }
        registry
            .definitions
            .borrow_mut()
            .insert(local.clone(), definition);
        let waiting = registry.waiting.borrow_mut().remove(&local);
        if let Some((promise, root)) = waiting {
            let result = agent.resolve_host_promise(promise, constructor);
            agent.unroot(root);
            result?;
        }
        let root = page.read(agent, page.main, |_, doc| Ok(doc.root()))?;
        upgrade_tree(agent, root, Some(&local))?;
        Ok(Value::UNDEFINED)
    })
}

fn read_definition(agent: &mut Agent, name: String, constructor: Value) -> JsResult<Definition> {
    let object = constructor.as_object().expect("constructor");
    let proto = agent.get(
        object,
        rustic_js::runtime::PropertyKey::String(agent.names.prototype),
    )?;
    let Some(proto) = proto.as_object() else {
        return agent.type_error("custom element constructor has no object prototype");
    };
    let roots = PrivateArray::new(agent, &[Value::object(proto)])?;
    with_root(agent, roots.value(), |agent| {
        let connected = callback(agent, proto, "connectedCallback")?;
        roots.set(agent, 1, connected)?;
        let disconnected = callback(agent, proto, "disconnectedCallback")?;
        roots.set(agent, 2, disconnected)?;
        let adopted = callback(agent, proto, "adoptedCallback")?;
        roots.set(agent, 3, adopted)?;
        let attribute_changed = callback(agent, proto, "attributeChangedCallback")?;
        roots.set(agent, 4, attribute_changed)?;
        let mut observed = Vec::new();
        if attribute_changed != Value::UNDEFINED {
            let key = agent.heap.key("observedAttributes")?;
            let value = agent.get(object, key)?;
            if value != Value::UNDEFINED {
                agent.iterate_values(value, |agent, value| {
                    agent.check_native_work()?;
                    observed.try_reserve(1).map_err(|_| Abrupt::OutOfMemory)?;
                    observed.push(creation::string(agent, value)?);
                    Ok(true)
                })?;
            }
        }
        Ok(Definition {
            name,
            constructor,
            connected,
            disconnected,
            adopted,
            attribute_changed,
            observed,
        })
    })
}

fn get(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    receiver(agent, this)?;
    let name = name(agent, args)?;
    with_page(agent, |_, page| {
        let definitions = page.custom_elements.definitions.borrow();
        Ok(name
            .as_ref()
            .and_then(|name| definitions.get(name))
            .map_or(Value::UNDEFINED, |def| def.constructor))
    })
}

fn get_name(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    receiver(agent, this)?;
    let constructor = required_arg(agent, args, 0)?;
    if !agent.is_constructor_value(constructor) {
        return agent.type_error("expected a constructor");
    }
    with_page(agent, |agent, page| {
        let definitions = page.custom_elements.definitions.borrow();
        match definitions
            .values()
            .find(|def| def.constructor == constructor)
        {
            Some(def) => agent.string(&def.name),
            None => Ok(Value::NULL),
        }
    })
}

fn when_defined(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    receiver(agent, this)?;
    let name = name(agent, args)?;
    let Some(name) = name.filter(|name| is_custom_element_name(name)) else {
        let error = match syntax_error(agent) {
            Abrupt::Throw(error) => error,
            other => return Err(other),
        };
        let promise = agent.new_host_promise()?;
        agent.reject_host_promise(promise, error);
        return Ok(Value::object(promise));
    };
    with_page(agent, |agent, page| {
        let registry = &page.custom_elements;
        if let Some(&(promise, _)) = registry.waiting.borrow().get(&name) {
            return Ok(Value::object(promise));
        }
        let promise = agent.new_host_promise()?;
        let value = Value::object(promise);
        let constructor = registry
            .definitions
            .borrow()
            .get(&name)
            .map(|def| def.constructor);
        if let Some(constructor) = constructor {
            with_root(agent, value, |agent| {
                agent.resolve_host_promise(promise, constructor)
            })?;
        } else {
            let root = agent.root(value)?;
            registry.waiting.borrow_mut().insert(name, (promise, root));
        }
        Ok(value)
    })
}

fn upgrade(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    receiver(agent, this)?;
    let root = required_arg(agent, args, 0)?;
    let root = node_of(agent, root)?;
    upgrade_tree(agent, root, None)?;
    Ok(Value::UNDEFINED)
}

pub(super) fn construct_html_element(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("HTMLElement requires a custom element constructor");
    };
    let constructor = Value::object(new_target);
    with_page(agent, |agent, page| {
        let registry = &page.custom_elements;
        let definition = registry
            .definitions
            .borrow()
            .values()
            .find(|def| def.constructor == constructor)
            .cloned();
        let Some(definition) = definition else {
            return agent
                .type_error("HTMLElement constructor is unavailable outside a custom element");
        };
        let proto = agent.host_prototype_from_new_target("HTMLElement", new_target)?;
        let existing = {
            let mut stack = registry.stack.borrow_mut();
            match stack
                .iter_mut()
                .rev()
                .find(|entry| entry.constructor == constructor)
            {
                Some(entry) => Some(entry.element.take()),
                None => None,
            }
        };
        match existing {
            Some(Some(element)) => {
                if !agent
                    .set_prototype_of(element.as_object().expect("node wrapper"), Some(proto))?
                {
                    return agent.type_error("cannot change custom element prototype");
                }
                Ok(element)
            }
            Some(None) => agent.type_error("custom element was already constructed"),
            None => {
                let name = QualifiedName::html(&definition.name).expect("definition name");
                let element =
                    create_node(agent, None, Some(proto), |doc| doc.create_element(name))?;
                agent.weak_map_set(registry.states, element, constructor)?;
                if definition.adopted != Value::UNDEFINED {
                    let owner = owner_document(agent, element)?;
                    agent.weak_map_set(registry.owners, element, owner)?;
                }
                Ok(element)
            }
        }
    })
}

pub(super) fn created(agent: &mut Agent, element: Value) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        if page.custom_elements.active() {
            let id = node_of(agent, element)?;
            if id.document() == page.main {
                try_upgrade(agent, page, id, None)?;
            }
        }
        Ok(element)
    })
}

fn candidates(agent: &mut Agent, root: NodeId) -> JsResult<Vec<NodeId>> {
    with_page(agent, |agent, page| {
        page.read(agent, root.document(), |agent, doc| {
            let mut result = Vec::new();
            let mut pending = vec![root];
            while let Some(id) = pending.pop() {
                agent.check_native_work()?;
                let Ok(node) = doc.node(id) else { continue };
                let mut child = node.last_child();
                while let Some(id) = child {
                    pending.try_reserve(1).map_err(|_| Abrupt::OutOfMemory)?;
                    pending.push(id);
                    child = doc.node(id).expect("live child").previous_sibling();
                }
                if let NodeKind::Element(element) = node.kind() {
                    if let Some(root) = element.shadow_root() {
                        pending.try_reserve(1).map_err(|_| Abrupt::OutOfMemory)?;
                        pending.push(root);
                    }
                    if element.name().namespace() == Some(HTML_NAMESPACE)
                        && is_custom_element_name(element.name().local())
                    {
                        result.try_reserve(1).map_err(|_| Abrupt::OutOfMemory)?;
                        result.push(id);
                    }
                }
            }
            Ok(result)
        })
    })
}

fn upgrade_tree(agent: &mut Agent, root: NodeId, name: Option<&str>) -> JsResult<()> {
    let candidates = candidates(agent, root)?;
    with_page(agent, |agent, page| {
        for id in candidates {
            try_upgrade(agent, page, id, name)?;
        }
        Ok(())
    })
}

fn try_upgrade(agent: &mut Agent, page: &Page, id: NodeId, only: Option<&str>) -> JsResult<bool> {
    let registry = &page.custom_elements;
    let data = read_doc(agent, id, |doc| {
        let NodeKind::Element(element) = doc.node(id).ok()?.kind() else {
            return None;
        };
        let local = element.name().local();
        if element.name().namespace() != Some(HTML_NAMESPACE)
            || only.is_some_and(|name| name != local)
        {
            return None;
        }
        let def = registry.definitions.borrow().get(local)?.clone();
        Some((
            def,
            element.attributes().to_vec(),
            super::events::connected(doc, id),
        ))
    })?;
    let Some((def, attributes, connected)) = data else {
        return Ok(false);
    };
    let target = wrap_node(agent, id)?;
    if agent.weak_map_get(registry.states, target) != Value::UNDEFINED {
        return Ok(false);
    }
    with_root(agent, target, |agent| {
        // A failed or currently upgrading element is never upgraded twice.
        agent.weak_map_set(registry.states, target, Value::NULL)?;
        registry.stack.borrow_mut().push(Construction {
            constructor: def.constructor,
            element: Some(target),
        });
        let result = agent.construct(def.constructor, &[]);
        let frame = registry
            .stack
            .borrow_mut()
            .pop()
            .expect("construction frame");
        match result {
            Ok(value) if value == target && frame.element.is_none() => {}
            Ok(_) => {
                let error = agent.throw(
                    rustic_js::runtime::ErrorKind::TypeError,
                    "custom element constructor must return its element",
                );
                report(agent, Err(error))?;
                return Ok(true);
            }
            Err(error) => {
                report(agent, Err(error))?;
                return Ok(true);
            }
        }
        agent.weak_map_set(registry.states, target, def.constructor)?;
        if def.adopted != Value::UNDEFINED {
            let owner = owner_document(agent, target)?;
            agent.weak_map_set(registry.owners, target, owner)?;
        }
        for attr in attributes {
            attribute_callback(agent, target, &def, attr.name(), None, Some(attr.value()))?;
        }
        if connected {
            call(agent, def.connected, target, &[])?;
        }
        Ok(true)
    })
}

fn report(agent: &mut Agent, result: JsResult<Value>) -> JsResult<()> {
    match result {
        Ok(_) => Ok(()),
        Err(Abrupt::Throw(error)) => super::events::report_callback_error(agent, error),
        Err(error) => Err(error),
    }
}

fn call(agent: &mut Agent, callback: Value, target: Value, args: &[Value]) -> JsResult<()> {
    if callback != Value::UNDEFINED {
        let result = agent.call(callback, target, args);
        report(agent, result)?;
    }
    Ok(())
}

fn attribute_callback(
    agent: &mut Agent,
    target: Value,
    def: &Definition,
    name: &QualifiedName,
    old: Option<&DomString>,
    new: Option<&DomString>,
) -> JsResult<()> {
    if !def.observed.iter().any(|attr| {
        attr.as_units()
            .iter()
            .copied()
            .eq(name.local().encode_utf16())
    }) {
        return Ok(());
    }
    let local = agent.string(name.local())?;
    let old = old.map_or(Ok(Value::NULL), |v| {
        crate::webidl::from_dom_string(agent, v)
    })?;
    let new = new.map_or(Ok(Value::NULL), |v| {
        crate::webidl::from_dom_string(agent, v)
    })?;
    let namespace = name
        .namespace()
        .map_or(Ok(Value::NULL), |v| agent.string(v))?;
    call(
        agent,
        def.attribute_changed,
        target,
        &[local, old, new, namespace],
    )
}

fn upgraded(agent: &Agent, registry: &Registry, target: Value) -> Option<Rc<Definition>> {
    let constructor = agent.weak_map_get(registry.states, target);
    registry
        .definitions
        .borrow()
        .values()
        .find(|def| def.constructor == constructor)
        .cloned()
}

fn owner_document(agent: &mut Agent, target: Value) -> JsResult<Value> {
    let id = node_of(agent, target)?;
    let root = read_doc(agent, id, |doc| doc.root())?;
    wrap_node(agent, root)
}

fn run_adopted(
    agent: &mut Agent,
    registry: &Registry,
    target: Value,
    def: &Definition,
) -> JsResult<()> {
    if def.adopted == Value::UNDEFINED {
        return Ok(());
    }
    let previous = agent.weak_map_get(registry.owners, target);
    let owner = owner_document(agent, target)?;
    if previous != owner {
        agent.weak_map_set(registry.owners, target, owner)?;
        if previous != Value::UNDEFINED {
            call(agent, def.adopted, target, &[previous, owner])?;
        }
    }
    Ok(())
}

pub(super) fn adopted(agent: &mut Agent, value: Value) -> JsResult<()> {
    with_page(agent, |agent, page| {
        let registry = &page.custom_elements;
        if registry.active() {
            let root = node_of(agent, value)?;
            for id in candidates(agent, root)? {
                let target = wrap_node(agent, id)?;
                if let Some(def) = upgraded(agent, registry, target) {
                    run_adopted(agent, registry, target, &def)?;
                }
            }
        }
        Ok(())
    })
}

pub(super) fn react(agent: &mut Agent, page: &Page, entries: &[MutationEntry]) -> JsResult<()> {
    let registry = &page.custom_elements;
    if !registry.active() {
        return Ok(());
    }
    for entry in entries {
        agent.check_native_work()?;
        match entry {
            MutationEntry::ChildList {
                target,
                added,
                removed,
                ..
            } => {
                let connected =
                    read_doc(agent, *target, |doc| super::events::connected(doc, *target))?;
                if !connected {
                    continue;
                }
                for (&node, connecting) in removed
                    .iter()
                    .map(|id| (id, false))
                    .chain(added.iter().map(|id| (id, true)))
                {
                    for id in candidates(agent, node)? {
                        if connecting && try_upgrade(agent, page, id, None)? {
                            continue;
                        }
                        let target = wrap_node(agent, id)?;
                        if let Some(def) = upgraded(agent, registry, target) {
                            if connecting {
                                run_adopted(agent, registry, target, &def)?;
                                call(agent, def.connected, target, &[])?;
                            } else {
                                call(agent, def.disconnected, target, &[])?;
                                run_adopted(agent, registry, target, &def)?;
                            }
                        }
                    }
                }
            }
            MutationEntry::Attribute {
                target,
                name,
                old_value,
            } => {
                let new = read_doc(agent, *target, |doc| {
                    let NodeKind::Element(element) = doc.node(*target).ok()?.kind() else {
                        return None;
                    };
                    if !registry
                        .definitions
                        .borrow()
                        .contains_key(element.name().local())
                    {
                        return None;
                    }
                    Some(
                        element
                            .attributes()
                            .iter()
                            .find(|attr| {
                                attr.name().local() == name.local()
                                    && attr.name().namespace() == name.namespace()
                            })
                            .map(|attr| attr.value().clone()),
                    )
                })?;
                if let Some(new) = new {
                    let target = wrap_node(agent, *target)?;
                    if let Some(def) = upgraded(agent, registry, target) {
                        attribute_callback(
                            agent,
                            target,
                            &def,
                            name,
                            old_value.as_ref(),
                            new.as_ref(),
                        )?;
                    }
                }
            }
            MutationEntry::CharacterData { .. } => {}
        }
    }
    Ok(())
}
