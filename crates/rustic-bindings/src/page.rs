//! The document host and the first live DOM bindings for page scripts.

use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};
use std::fmt::Debug;
use std::rc::Rc;

use rustic_dom::{Document, DomError, Node, NodeId, NodeKind};
use rustic_js::runtime::{
    Agent, ExecutionHost, JsResult, NativeFn, ObjectId, OutOfMemory, PropertyDescriptor,
    PropertyKey, Roots, Value,
};

use crate::{DomGcError, DomHeap, dom_exception};

/// Shared access to the real document rendered by the host.
pub trait PageTree: Debug {
    /// Borrow the document for reading.
    fn read(&self) -> Option<Ref<'_, Document>>;
    /// Borrow the document for a checked edit or collection.
    fn write(&self) -> Option<RefMut<'_, Document>>;
}

#[derive(Debug)]
struct Page {
    tree: Rc<dyn PageTree>,
    wrappers: RefCell<DomHeap>,
}

impl ExecutionHost for Page {
    fn collect(&self, agent: &mut Agent) -> Result<(), OutOfMemory> {
        let mut document = self.tree.write().ok_or(OutOfMemory)?;
        self.wrappers
            .try_borrow_mut()
            .map_err(|_| OutOfMemory)?
            .collect(agent, &mut document, Roots::default())
            .map_err(|_| OutOfMemory)?;
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Attach joint collection and install the root of a host's document in the
/// running realm. The host installs [`dom_exception`] before this entry point.
pub fn install(agent: &mut Agent, tree: Rc<dyn PageTree>) -> JsResult<()> {
    if agent.execution_host().is_some() {
        return agent.type_error("an execution host is already attached");
    }
    let Some(document) = tree.read() else {
        return agent.type_error("document is busy");
    };
    install_interface(
        agent,
        "Node",
        agent.intrinsics.object_prototype,
        &[
            ("nodeType", "get nodeType", node_type),
            ("parentNode", "get parentNode", parent_node),
            ("firstChild", "get firstChild", first_child),
            ("nextSibling", "get nextSibling", next_sibling),
            ("ownerDocument", "get ownerDocument", owner_document),
        ],
        |agent, prototype| {
            for (name, behavior) in [
                ("appendChild", append_child as NativeFn),
                ("removeChild", remove_child),
            ] {
                let key = agent.heap.key(name)?;
                agent.define_method(prototype, key, behavior, name, 1)?;
            }
            Ok(())
        },
    )?;
    let node_prototype = prototype(agent, "Node")?;
    install_interface(
        agent,
        "Document",
        node_prototype,
        &[("documentElement", "get documentElement", document_element)],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        "Element",
        node_prototype,
        &[("localName", "get localName", local_name)],
        |_, _| Ok(()),
    )?;

    let mut wrappers = DomHeap::new(&document);
    let root = wrap(&mut wrappers, agent, &document, document.root())?;
    drop(document);
    let key = agent.heap.key("document")?;
    agent.define_property_or_throw(
        agent.intrinsics.global,
        key,
        PropertyDescriptor::data_with(root, false, true, false),
    )?;
    agent.attach_execution_host(Rc::new(Page {
        tree,
        wrappers: RefCell::new(wrappers),
    }))
}

fn install_interface(
    agent: &mut Agent,
    interface: &'static str,
    parent: ObjectId,
    attributes: &[(&str, &str, NativeFn)],
    initialize: impl FnOnce(&mut Agent, ObjectId) -> JsResult<()>,
) -> JsResult<()> {
    let prototype = agent.new_object_with(Some(parent))?;
    let tag = Value::string(agent.heap.intern_str(interface)?);
    agent.define_property_or_throw(
        prototype,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    for &(name, getter_name, behavior) in attributes {
        let getter = Value::object(agent.new_native_function(behavior, getter_name, 0)?);
        let key = agent.heap.key(name)?;
        agent.define_property_or_throw(
            prototype,
            key,
            PropertyDescriptor::accessor(getter, Value::UNDEFINED, true, true),
        )?;
    }
    initialize(agent, prototype)?;
    let parent_constructor = if interface == "Node" {
        None
    } else {
        let key = agent.heap.key("Node")?;
        agent.get(agent.intrinsics.global, key)?.as_object()
    };
    agent.install_host_interface(
        interface,
        illegal_constructor,
        0,
        prototype,
        |agent, constructor| {
            if let Some(parent) = parent_constructor {
                agent.set_prototype_of(constructor, Some(parent))?;
            }
            Ok(())
        },
    )
}

fn illegal_constructor(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    agent.type_error("illegal DOM constructor")
}

fn prototype(agent: &mut Agent, interface: &'static str) -> JsResult<ObjectId> {
    match agent.host_interface_prototype(interface) {
        Some(prototype) => Ok(prototype),
        None => agent.type_error("DOM interface is not installed"),
    }
}

fn wrap(
    wrappers: &mut DomHeap,
    agent: &mut Agent,
    document: &Document,
    id: NodeId,
) -> JsResult<Value> {
    let node = document
        .node(id)
        .map_err(|error| dom_exception::from_dom_error(agent, error))?;
    let interface = match node.kind() {
        NodeKind::Document => "Document",
        NodeKind::Element(_) => "Element",
        _ => "Node",
    };
    let proto = prototype(agent, interface)?;
    wrappers
        .wrap_with_prototype(agent, document, id, proto)
        .map(Value::object)
        .map_err(|error| match error {
            DomGcError::Dom(error) => dom_exception::from_dom_error(agent, error),
            _ => agent.throw(
                rustic_js::runtime::ErrorKind::TypeError,
                "node is outside this document host",
            ),
        })
}

fn with_page<T>(
    agent: &mut Agent,
    operation: impl FnOnce(&mut Agent, &Page) -> JsResult<T>,
) -> JsResult<T> {
    let host = agent.execution_host();
    let Some(page) = host
        .as_ref()
        .and_then(|host| host.as_any().downcast_ref::<Page>())
    else {
        return agent.type_error("no DOM document is attached");
    };
    operation(agent, page)
}

fn node_id(
    agent: &mut Agent,
    wrappers: &DomHeap,
    document: &Document,
    value: Value,
) -> JsResult<NodeId> {
    match value
        .as_object()
        .and_then(|object| wrappers.node_of(agent, document, object))
    {
        Some(id) => Ok(id),
        None => agent.type_error("expected a live DOM Node"),
    }
}

fn read_node(
    agent: &mut Agent,
    receiver: Value,
    read: impl FnOnce(&mut Agent, &mut DomHeap, &Document, &Node) -> JsResult<Value>,
) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        let Some(document) = page.tree.read() else {
            return agent.type_error("document is busy");
        };
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        let id = node_id(agent, &wrappers, &document, receiver)?;
        let node = document
            .node(id)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        // These operations never run script while the tree or cache is borrowed.
        read(agent, &mut wrappers, &document, node)
    })
}

fn node_type(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |_, _, _, node| {
        let kind = match node.kind() {
            NodeKind::Element(_) => 1,
            NodeKind::Text(_) => 3,
            NodeKind::CdataSection(_) => 4,
            NodeKind::ProcessingInstruction(_) => 7,
            NodeKind::Comment(_) => 8,
            NodeKind::Document => 9,
            NodeKind::DocumentType(_) => 10,
            NodeKind::DocumentFragment => 11,
        };
        Ok(Value::number(f64::from(kind)))
    })
}

fn related(
    agent: &mut Agent,
    receiver: Value,
    relation: impl FnOnce(&Document, &Node) -> Option<NodeId>,
) -> JsResult<Value> {
    read_node(
        agent,
        receiver,
        |agent, wrappers, document, node| match relation(document, node) {
            Some(other) => wrap(wrappers, agent, document, other),
            None => Ok(Value::NULL),
        },
    )
}

fn first_child(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    related(agent, receiver, |_, node| node.first_child())
}

fn next_sibling(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    related(agent, receiver, |_, node| node.next_sibling())
}

fn parent_node(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    related(agent, receiver, |_, node| node.parent())
}

fn owner_document(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    related(agent, receiver, |document, node| {
        (!matches!(node.kind(), NodeKind::Document)).then(|| document.root())
    })
}

fn document_element(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, wrappers, document, node| {
        if !matches!(node.kind(), NodeKind::Document) {
            return agent.type_error("expected a Document receiver");
        }
        let mut current = node.first_child();
        while let Some(id) = current {
            let child = document
                .node(id)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
            if matches!(child.kind(), NodeKind::Element(_)) {
                return wrap(wrappers, agent, document, id);
            }
            current = child.next_sibling();
        }
        Ok(Value::NULL)
    })
}

fn local_name(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, _, node| match node.kind() {
        NodeKind::Element(element) => agent.string(element.name().local()),
        _ => agent.type_error("expected an Element receiver"),
    })
}

fn edit_child(
    agent: &mut Agent,
    receiver: Value,
    args: &[Value],
    edit: impl FnOnce(&mut Document, NodeId, NodeId) -> Result<(), DomError>,
) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        let Some(mut document) = page.tree.write() else {
            return agent.type_error("document is busy");
        };
        let Ok(wrappers) = page.wrappers.try_borrow() else {
            return agent.type_error("DOM bindings are busy");
        };
        let parent = node_id(agent, &wrappers, &document, receiver)?;
        let value = args.first().copied().unwrap_or(Value::UNDEFINED);
        let child = node_id(agent, &wrappers, &document, value)?;
        edit(&mut document, parent, child)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        Ok(value)
    })
}

fn append_child(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    edit_child(agent, receiver, args, Document::append_child)
}

fn remove_child(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    edit_child(agent, receiver, args, |document, parent, child| {
        if document.node(child)?.parent() != Some(parent) {
            return Err(DomError::InvalidReference);
        }
        document.detach(child)
    })
}
