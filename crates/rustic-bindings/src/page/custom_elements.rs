//! The Window custom-element registry and reactions for autonomous elements.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rustic_dom::{
    DomString, HTML_NAMESPACE, MutationEntry, NodeId, NodeKind, QualifiedName,
    is_custom_element_name,
};
use rustic_js::runtime::{Abrupt, Agent, JsResult, ObjectId, Value};

use super::events::{PrivateArray, define_method, with_root};
use super::traversal::{node_of, read_doc, wrap_node};
use super::{Page, create_node, creation, install_interface, prototype, required_arg, with_page};

pub(super) mod builtins;
pub(super) mod internals;
mod scopes;

pub(super) use scopes::{
    check_global, convert_registry, copy_scopes, owner_registry, registry_of, set_registry,
    upgrade_copy,
};

const INTERFACE: &str = "CustomElementRegistry";

#[derive(Debug)]
struct Definition {
    name: String,
    local: String,
    constructor: Value,
    connected: Value,
    disconnected: Value,
    adopted: Value,
    attribute_changed: Value,
    observed: Vec<DomString>,
    disable_internals: bool,
    disable_shadow: bool,
    forms: Option<internals::Forms>,
}

#[derive(Debug)]
struct Construction {
    constructor: Value,
    target: Value,
    element: Option<Value>,
    registry: Value,
}

#[derive(Debug)]
struct Definitions {
    definitions: RefCell<HashMap<String, Rc<Definition>>>,
    roots: PrivateArray,
    waiting: RefCell<HashMap<String, (ObjectId, u32)>>,
    defining: Cell<bool>,
}

impl Definitions {
    fn new(roots: PrivateArray) -> Self {
        Self {
            definitions: RefCell::new(HashMap::new()),
            roots,
            waiting: RefCell::new(HashMap::new()),
            defining: Cell::new(false),
        }
    }

    fn by_constructor(&self, constructor: Value) -> Option<Rc<Definition>> {
        self.definitions
            .borrow()
            .values()
            .find(|def| def.constructor == constructor)
            .cloned()
    }
}

#[derive(Debug)]
pub(super) struct Registry {
    object: Value,
    global: Rc<Definitions>,
    scoped: RefCell<HashMap<ObjectId, Rc<Definitions>>>,
    roots: ObjectId,
    assignments: ObjectId,
    states: ObjectId,
    definition_scopes: ObjectId,
    owners: ObjectId,
    stack: RefCell<Vec<Construction>>,
    active: Cell<bool>,
    has_form_callbacks: Cell<bool>,
    internals_targets: ObjectId,
    attached: ObjectId,
    form_data: ObjectId,
}

impl Registry {
    pub(super) fn active(&self) -> bool {
        self.active.get()
    }

    fn data(&self, agent: &Agent, scope: Value) -> Option<Rc<Definitions>> {
        if scope == self.object {
            return Some(self.global.clone());
        }
        let id = scope.as_object()?;
        if !agent.weak_map_has(self.roots, scope) {
            return None;
        }
        self.scoped.borrow().get(&id).cloned()
    }
}

pub(super) fn install(agent: &mut Agent) -> JsResult<Registry> {
    builtins::install(agent)?;
    internals::install(agent)?;
    install_interface(
        agent,
        INTERFACE,
        None,
        scopes::construct,
        0,
        &[],
        |agent, proto| {
            define_method(agent, proto, "define", define, 2)?;
            define_method(agent, proto, "get", get, 1)?;
            define_method(agent, proto, "getName", get_name, 1)?;
            define_method(agent, proto, "whenDefined", when_defined, 1)?;
            define_method(agent, proto, "upgrade", upgrade, 1)?;
            define_method(agent, proto, "initialize", scopes::initialize, 1)
        },
    )?;
    let proto = prototype(agent, INTERFACE)?;
    let object = Value::object(agent.new_object_with(Some(proto))?);
    let global_roots = PrivateArray::new(agent, &[])?;
    let roots = agent.new_weak_map()?;
    let assignments = agent.new_weak_map()?;
    let states = agent.new_weak_map()?;
    let definition_scopes = agent.new_weak_map()?;
    let owners = agent.new_weak_map()?;
    let internals_targets = agent.new_weak_map()?;
    let attached = agent.new_weak_map()?;
    let form_data = agent.new_weak_map()?;
    let memory = PrivateArray::new(
        agent,
        &[
            object,
            global_roots.value(),
            Value::object(roots),
            Value::object(assignments),
            Value::object(states),
            Value::object(definition_scopes),
            Value::object(owners),
            Value::object(internals_targets),
            Value::object(attached),
            Value::object(form_data),
        ],
    )?;
    agent.root(memory.value())?;
    let global = agent.intrinsics.global;
    super::elements::accessor(agent, global, "customElements", registry, None)?;
    Ok(Registry {
        object,
        global: Rc::new(Definitions::new(global_roots)),
        scoped: RefCell::new(HashMap::new()),
        roots,
        assignments,
        states,
        definition_scopes,
        owners,
        stack: RefCell::new(Vec::new()),
        active: Cell::new(false),
        has_form_callbacks: Cell::new(false),
        internals_targets,
        attached,
        form_data,
    })
}

fn registry(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    with_page(agent, |_, page| Ok(page.custom_elements.object))
}

fn receiver(agent: &mut Agent, value: Value) -> JsResult<Rc<Definitions>> {
    with_page(agent, |agent, page| {
        page.custom_elements.data(agent, value).ok_or_else(|| {
            agent.throw(
                rustic_js::runtime::ErrorKind::TypeError,
                "expected CustomElementRegistry",
            )
        })
    })
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
    let definitions = receiver(agent, this)?;
    let name = name(agent, args)?
        .filter(|name| is_custom_element_name(name))
        .ok_or_else(|| syntax_error(agent))?;
    let constructor = required_arg(agent, args, 1)?;
    if !agent.is_constructor_value(constructor) {
        return agent.type_error("custom element definition requires a constructor");
    }
    let mut local = name.clone();
    if let Some(options) = args.get(2).and_then(|value| value.as_object()) {
        let key = agent.heap.key("extends")?;
        let value = agent.get(options, key)?;
        if value != Value::UNDEFINED {
            let tag = creation::string(agent, value)?;
            local = creation::scalar_name(&tag)?
                .filter(|tag| builtins::extends(tag).is_some())
                .ok_or_else(|| unsupported(agent, "unknown built-in element"))?;
        }
    }
    with_page(agent, |agent, page| {
        let registry = &page.custom_elements;
        if local != name && this != registry.object {
            return Err(unsupported(
                agent,
                "scoped registries cannot customize built-ins",
            ));
        }
        {
            let definitions = definitions.definitions.borrow();
            if definitions.contains_key(&name)
                || definitions
                    .values()
                    .any(|def| def.constructor == constructor)
            {
                return Err(unsupported(agent, "duplicate custom element definition"));
            }
        }
        if definitions.defining.replace(true) {
            return Err(unsupported(
                agent,
                "a custom element definition is already being read",
            ));
        }
        let result = read_definition(agent, name.clone(), local, constructor);
        definitions.defining.set(false);
        let definition = Rc::new(result?);
        super::mutation_observer::enable_journals(agent, page)?;
        for value in [
            constructor,
            definition.connected,
            definition.disconnected,
            definition.adopted,
            definition.attribute_changed,
        ] {
            let index = definitions.roots.len(agent)?;
            definitions.roots.set(agent, index, value)?;
        }
        if let Some(forms) = &definition.forms {
            for value in [forms.associated, forms.disabled, forms.reset, forms.restore] {
                let index = definitions.roots.len(agent)?;
                definitions.roots.set(agent, index, value)?;
            }
            if forms.associated != Value::UNDEFINED || forms.disabled != Value::UNDEFINED {
                registry.has_form_callbacks.set(true);
            }
        }
        definitions
            .definitions
            .borrow_mut()
            .insert(name.clone(), definition);
        registry.active.set(true);
        let waiting = definitions.waiting.borrow_mut().remove(&name);
        if let Some((promise, index)) = waiting {
            agent.resolve_host_promise(promise, constructor)?;
            definitions.roots.set(agent, index, Value::UNDEFINED)?;
        }
        if this == registry.object {
            let root = page.read(agent, page.main, |_, doc| Ok(doc.root()))?;
            upgrade_tree(agent, root, Some(&name), this)?;
        } else {
            scopes::definition_added(agent, page, this, &name)?;
        }
        Ok(Value::UNDEFINED)
    })
}

fn read_definition(
    agent: &mut Agent,
    name: String,
    local: String,
    constructor: Value,
) -> JsResult<Definition> {
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
        let key = agent.heap.key("disabledFeatures")?;
        let features = agent.get(object, key)?;
        let mut disable_shadow = false;
        let mut disable_internals = false;
        if features != Value::UNDEFINED {
            agent.iterate_values(features, |agent, feature| {
                agent.check_native_work()?;
                let feature = creation::string(agent, feature)?;
                if feature == DomString::from("internals") {
                    disable_internals = true;
                } else if feature == DomString::from("shadow") {
                    disable_shadow = true;
                }
                Ok(true)
            })?;
        }
        let key = agent.heap.key("formAssociated")?;
        let form_associated = agent.get(object, key)?;
        let forms = if local == name && agent.to_boolean(form_associated) {
            let associated = callback(agent, proto, "formAssociatedCallback")?;
            roots.set(agent, 5, associated)?;
            let disabled = callback(agent, proto, "formDisabledCallback")?;
            roots.set(agent, 6, disabled)?;
            let reset = callback(agent, proto, "formResetCallback")?;
            roots.set(agent, 7, reset)?;
            let restore = callback(agent, proto, "formStateRestoreCallback")?;
            roots.set(agent, 8, restore)?;
            Some(internals::Forms {
                associated,
                disabled,
                reset,
                restore,
            })
        } else {
            None
        };
        Ok(Definition {
            name,
            local,
            constructor,
            connected,
            disconnected,
            adopted,
            attribute_changed,
            observed,
            disable_internals,
            disable_shadow,
            forms,
        })
    })
}

fn get(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let registry = receiver(agent, this)?;
    let name = name(agent, args)?;
    let definitions = registry.definitions.borrow();
    Ok(name
        .as_ref()
        .and_then(|name| definitions.get(name))
        .map_or(Value::UNDEFINED, |def| def.constructor))
}

fn get_name(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let registry = receiver(agent, this)?;
    let constructor = required_arg(agent, args, 0)?;
    if !agent.is_constructor_value(constructor) {
        return agent.type_error("expected a constructor");
    }
    match registry.by_constructor(constructor) {
        Some(def) => agent.string(&def.name),
        None => Ok(Value::NULL),
    }
}

fn when_defined(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let registry = receiver(agent, this)?;
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
        let mut waiting = registry.waiting.borrow_mut();
        waiting.try_reserve(1).map_err(|_| Abrupt::OutOfMemory)?;
        let index = registry.roots.len(agent)?;
        registry.roots.set(agent, index, value)?;
        waiting.insert(name, (promise, index));
    }
    Ok(value)
}

fn upgrade(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    receiver(agent, this)?;
    let root = required_arg(agent, args, 0)?;
    let root = node_of(agent, root)?;
    upgrade_tree(agent, root, None, this)?;
    Ok(Value::UNDEFINED)
}

fn construct_element(agent: &mut Agent, interface: &'static str) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("HTMLElement requires a custom element constructor");
    };
    if agent.active_function() == Some(new_target) {
        return agent.type_error("HTML interface cannot construct itself");
    }
    let constructor = Value::object(new_target);
    with_page(agent, |agent, page| {
        let registry = &page.custom_elements;
        let scope = registry
            .stack
            .borrow()
            .iter()
            .rev()
            .find(|entry| entry.constructor == constructor)
            .map_or(registry.object, |entry| entry.registry);
        let definition = registry
            .data(agent, scope)
            .and_then(|registry| registry.by_constructor(constructor));
        let Some(definition) = definition else {
            return agent
                .type_error("HTMLElement constructor is unavailable outside a custom element");
        };
        if builtins::wrapper(&definition.local) != interface {
            return agent.type_error("custom element uses the wrong HTML base constructor");
        }
        let proto = agent.host_prototype_from_new_target(interface, new_target)?;
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
                let name = QualifiedName::html(&definition.local).expect("definition local name");
                let custom_is = (definition.local != definition.name)
                    .then(|| DomString::from(definition.name.as_str()));
                let element = create_node(agent, None, Some(proto), |doc| {
                    creation::element_with_is(doc, name, custom_is)
                })?;
                agent.weak_map_set(registry.assignments, element, scope)?;
                record_upgrade(agent, registry, element, scope, constructor)?;
                if definition.forms.is_some() {
                    internals::mark_associated(agent, element)?;
                }
                if definition.adopted != Value::UNDEFINED {
                    let owner = owner_document(agent, element)?;
                    agent.weak_map_set(registry.owners, element, owner)?;
                }
                Ok(element)
            }
        }
    })
}

pub(super) fn created(agent: &mut Agent, element: Value, scope: Option<Value>) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        let scope = match scope {
            Some(scope) => Some(scope),
            None => {
                let scope = registry_of(agent, element, &[])?;
                (scope != Value::NULL).then_some(scope)
            }
        };
        if let Some(scope) = scope {
            set_registry(agent, element, scope)?;
        }
        if page.custom_elements.active() {
            let id = node_of(agent, element)?;
            try_upgrade(agent, page, id, None, None)?;
        }
        Ok(element)
    })
}

fn definition_name(element: &rustic_dom::Element) -> Option<std::borrow::Cow<'_, str>> {
    if element.name().namespace() != Some(HTML_NAMESPACE) {
        return None;
    }
    let local = element.name().local();
    if is_custom_element_name(local) {
        return Some(std::borrow::Cow::Borrowed(local));
    }
    String::from_utf16(element.custom_is()?.as_units())
        .ok()
        .map(std::borrow::Cow::Owned)
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
                    if definition_name(element).is_some() {
                        result.try_reserve(1).map_err(|_| Abrupt::OutOfMemory)?;
                        result.push(id);
                    }
                }
            }
            Ok(result)
        })
    })
}

fn upgrade_tree(agent: &mut Agent, root: NodeId, name: Option<&str>, scope: Value) -> JsResult<()> {
    let candidates = candidates(agent, root)?;
    with_page(agent, |agent, page| {
        for id in candidates {
            try_upgrade(agent, page, id, name, Some(scope))?;
        }
        Ok(())
    })
}

fn try_upgrade(
    agent: &mut Agent,
    page: &Page,
    id: NodeId,
    only: Option<&str>,
    scope_filter: Option<Value>,
) -> JsResult<bool> {
    let registry = &page.custom_elements;
    let target = wrap_node(agent, id)?;
    if agent.weak_map_has(registry.states, target) {
        return Ok(false);
    }
    let scope = registry_of(agent, target, &[])?;
    if scope_filter.is_some_and(|filter| filter != scope) {
        return Ok(false);
    }
    let Some(definitions) = registry.data(agent, scope) else {
        return Ok(false);
    };
    let data = read_doc(agent, id, |doc| {
        let NodeKind::Element(element) = doc.node(id).ok()?.kind() else {
            return None;
        };
        let local = element.name().local();
        let name = definition_name(element)?;
        if only.is_some_and(|only| only != name) {
            return None;
        }
        let def = definitions.definitions.borrow().get(name.as_ref())?.clone();
        if def.local != local {
            return None;
        }
        Some((
            def,
            element.attributes().to_vec(),
            super::events::connected(doc, id),
        ))
    })?;
    let Some((def, attributes, connected)) = data else {
        return Ok(false);
    };
    with_root(agent, target, |agent| {
        // A failed or currently upgrading element is never upgraded twice.
        agent.weak_map_set(registry.states, target, Value::NULL)?;
        if def.disable_shadow
            && read_doc(
                agent,
                id,
                |document| matches!(document.node(id).expect("live element").kind(), NodeKind::Element(element) if element.shadow_root().is_some()),
            )?
        {
            let error = unsupported(agent, "custom element definition disables shadow");
            report(agent, Err(error))?;
            return Ok(true);
        }
        agent.weak_map_set(registry.assignments, target, scope)?;
        registry
            .stack
            .borrow_mut()
            .try_reserve(1)
            .map_err(|_| Abrupt::OutOfMemory)?;
        registry.stack.borrow_mut().push(Construction {
            constructor: def.constructor,
            target,
            element: Some(target),
            registry: scope,
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
        record_upgrade(agent, registry, target, scope, def.constructor)?;
        if def.forms.is_some() {
            internals::mark_associated(agent, target)?;
        }
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
        internals::refresh(agent, target, &def)?;
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

fn record_upgrade(
    agent: &mut Agent,
    registry: &Registry,
    target: Value,
    scope: Value,
    constructor: Value,
) -> JsResult<()> {
    agent.weak_map_set(registry.definition_scopes, target, scope)?;
    agent.weak_map_set(registry.states, target, constructor)
}

fn upgraded(agent: &Agent, registry: &Registry, target: Value) -> Option<Rc<Definition>> {
    let scope = agent.weak_map_get(registry.definition_scopes, target);
    let constructor = agent.weak_map_get(registry.states, target);
    registry.data(agent, scope)?.by_constructor(constructor)
}

fn current_definition(agent: &Agent, registry: &Registry, target: Value) -> Option<Rc<Definition>> {
    upgraded(agent, registry, target).or_else(|| {
        let stack = registry.stack.borrow();
        let entry = stack
            .iter()
            .rev()
            .find(|entry| entry.target == target && entry.element.is_none())?;
        registry
            .data(agent, entry.registry)?
            .by_constructor(entry.constructor)
    })
}

pub(super) fn shadow_allowed(agent: &mut Agent, target: Value) -> JsResult<bool> {
    with_page(agent, |agent, page| {
        let registry = &page.custom_elements;
        if let Some(def) = current_definition(agent, registry, target) {
            return Ok(!def.disable_shadow);
        }
        let id = node_of(agent, target)?;
        let scope = registry_of(agent, target, &[])?;
        let Some(definitions) = registry.data(agent, scope) else {
            return Ok(true);
        };
        read_doc(agent, id, |doc| {
            let NodeKind::Element(element) = doc.node(id).expect("live target").kind() else {
                return true;
            };
            let Some(name) = definition_name(element) else {
                return true;
            };
            !definitions
                .definitions
                .borrow()
                .get(name.as_ref())
                .is_some_and(|def| def.local == element.name().local() && def.disable_shadow)
        })
    })
}

pub(super) fn is_constructed(agent: &mut Agent, target: Value) -> JsResult<bool> {
    with_page(agent, |agent, page| {
        Ok(current_definition(agent, &page.custom_elements, target).is_some())
    })
}

pub(super) fn owner_document(agent: &mut Agent, target: Value) -> JsResult<Value> {
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
                    internals::refresh(agent, target, &def)?;
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
                connected,
                added,
                removed,
                ..
            } => {
                if !connected {
                    continue;
                }
                for (&node, connecting) in removed
                    .iter()
                    .map(|id| (id, false))
                    .chain(added.iter().map(|id| (id, true)))
                {
                    for id in candidates(agent, node)? {
                        if connecting && try_upgrade(agent, page, id, None, None)? {
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
                            internals::refresh(agent, target, &def)?;
                        }
                    }
                }
            }
            MutationEntry::Attribute {
                target,
                name,
                old_value,
                new_value,
            } => {
                let Some(wrapper) = page
                    .wrappers
                    .try_borrow()
                    .ok()
                    .and_then(|wrappers| wrappers.existing_wrapper(*target))
                else {
                    continue;
                };
                let target = Value::object(wrapper);
                if let Some(def) = upgraded(agent, registry, target) {
                    attribute_callback(
                        agent,
                        target,
                        &def,
                        name,
                        old_value.as_ref(),
                        new_value.as_ref(),
                    )?;
                }
            }
            MutationEntry::CharacterData { .. } => {}
        }
    }
    if registry.has_form_callbacks.get() && !entries.is_empty() {
        // External form owners and disabled fieldsets can affect elements
        // outside the immediate mutation target.
        let mut documents = Vec::new();
        for entry in entries {
            let id = entry.target().document();
            if !documents.contains(&id) {
                documents.try_reserve(1).map_err(|_| Abrupt::OutOfMemory)?;
                documents.push(id);
            }
        }
        for document in documents {
            let root = page.read(agent, document, |_, doc| Ok(doc.root()))?;
            for id in candidates(agent, root)? {
                let target = wrap_node(agent, id)?;
                if let Some(def) = upgraded(agent, registry, target) {
                    internals::refresh(agent, target, &def)?;
                }
            }
        }
    }
    Ok(())
}
