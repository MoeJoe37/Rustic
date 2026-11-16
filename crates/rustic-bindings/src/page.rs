//! The document host and the first live DOM bindings for page scripts.

use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::fmt::Debug;
use std::rc::Rc;

use rustic_dom::{Document, DomError, Node, NodeId, NodeKind};
use rustic_js::runtime::{
    Agent, ExecutionHost, JsResult, NativeFn, ObjectId, OutOfMemory, PropertyDescriptor,
    PropertyKey, RealmId, Roots, Value,
};

use crate::{DomGcError, DomHeap, dom_exception};

mod attributes;
mod character_data;
mod collections;
mod creation;
mod html_collections;
mod selectors;

/// Shared access to the real document rendered by the host.
pub trait PageTree: Debug {
    /// Borrow the document for reading.
    fn read(&self) -> Option<Ref<'_, Document>>;
    /// Borrow the document for a checked edit or collection.
    fn write(&self) -> Option<RefMut<'_, Document>>;
    /// Whether ID and class selectors use HTML quirks matching.
    fn quirks_mode(&self) -> bool {
        false
    }
}

#[derive(Debug)]
struct Page {
    tree: Rc<dyn PageTree>,
    wrappers: RefCell<DomHeap>,
    child_lists: RefCell<HashMap<(NodeId, RealmId, collections::ChildKind), ObjectId>>,
    child_index: RefCell<collections::ChildIndex>,
}

impl ExecutionHost for Page {
    fn collect(&self, agent: &mut Agent) -> Result<(), OutOfMemory> {
        let mut document = self.tree.write().ok_or(OutOfMemory)?;
        self.wrappers
            .try_borrow_mut()
            .map_err(|_| OutOfMemory)?
            .collect(agent, &mut document, Roots::default())
            .map_err(|_| OutOfMemory)?;
        self.child_lists
            .try_borrow_mut()
            .map_err(|_| OutOfMemory)?
            .retain(|(node, _, _), _| document.node(*node).is_ok());
        self.child_index
            .try_borrow_mut()
            .map_err(|_| OutOfMemory)?
            .discard_collected(&document);
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
    collections::install(agent)?;
    html_collections::install(agent)?;
    install_interface(
        agent,
        "Node",
        None,
        illegal_constructor,
        0,
        &[
            ("nodeType", "get nodeType", node_type),
            ("parentNode", "get parentNode", parent_node),
            ("firstChild", "get firstChild", first_child),
            ("nextSibling", "get nextSibling", next_sibling),
            ("ownerDocument", "get ownerDocument", owner_document),
            ("childNodes", "get childNodes", collections::child_nodes),
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
    install_interface(
        agent,
        "Document",
        Some("Node"),
        illegal_constructor,
        0,
        &[
            ("documentElement", "get documentElement", document_element),
            (
                "children",
                "get children",
                html_collections::document_children,
            ),
        ],
        |agent, proto| {
            creation::install_factories(agent, proto)?;
            selectors::install_parent_node(agent, proto)?;
            selectors::install_non_element_parent(agent, proto)?;
            html_collections::install_lookup::<true>(agent, proto)
        },
    )?;
    install_interface(
        agent,
        "Element",
        Some("Node"),
        illegal_constructor,
        0,
        &[
            ("localName", "get localName", local_name),
            (
                "children",
                "get children",
                html_collections::element_children,
            ),
        ],
        |agent, proto| {
            attributes::install(agent, proto)?;
            selectors::install_parent_node(agent, proto)?;
            selectors::install_element(agent, proto)?;
            html_collections::install_lookup::<false>(agent, proto)
        },
    )?;
    install_interface(
        agent,
        "DocumentFragment",
        Some("Node"),
        creation::construct_fragment,
        0,
        &[(
            "children",
            "get children",
            html_collections::fragment_children,
        )],
        |agent, proto| {
            selectors::install_parent_node(agent, proto)?;
            selectors::install_non_element_parent(agent, proto)
        },
    )?;
    character_data::install(agent)?;
    install_interface(
        agent,
        "ProcessingInstruction",
        Some("CharacterData"),
        creation::construct_instruction,
        1,
        &[("target", "get target", creation::instruction_target)],
        |_, _| Ok(()),
    )?;

    install_interface(
        agent,
        "DocumentType",
        Some("Node"),
        illegal_constructor,
        0,
        &[
            ("name", "get name", creation::doctype_name),
            ("publicId", "get publicId", creation::doctype_public_id),
            ("systemId", "get systemId", creation::doctype_system_id),
        ],
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
        child_lists: RefCell::new(HashMap::new()),
        child_index: RefCell::new(collections::ChildIndex::default()),
    }))
}

fn install_interface(
    agent: &mut Agent,
    interface: &'static str,
    parent: Option<&'static str>,
    constructor: NativeFn,
    length: u32,
    attributes: &[(&str, &str, NativeFn)],
    initialize: impl FnOnce(&mut Agent, ObjectId) -> JsResult<()>,
) -> JsResult<()> {
    let (parent_prototype, parent_constructor) = match parent {
        Some(parent) => {
            let prototype = prototype(agent, parent)?;
            let key = PropertyKey::String(agent.names.constructor);
            (prototype, agent.get(prototype, key)?.as_object())
        }
        None => (agent.intrinsics.object_prototype, None),
    };
    let prototype = agent.new_object_with(Some(parent_prototype))?;
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
    agent.install_host_interface(
        interface,
        constructor,
        length,
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
        NodeKind::Text(_) => "Text",
        NodeKind::Comment(_) => "Comment",
        NodeKind::CdataSection(_) => "CDATASection",
        NodeKind::ProcessingInstruction(_) => "ProcessingInstruction",
        NodeKind::DocumentFragment => "DocumentFragment",
        NodeKind::DocumentType(_) => "DocumentType",
    };
    let proto = prototype(agent, interface)?;
    wrap_using(wrappers, agent, document, id, proto)
}

fn wrap_using(
    wrappers: &mut DomHeap,
    agent: &mut Agent,
    document: &Document,
    id: NodeId,
    proto: ObjectId,
) -> JsResult<Value> {
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

fn require_receiver(
    agent: &mut Agent,
    receiver: Value,
    brand: fn(&NodeKind) -> bool,
    interface: &str,
) -> JsResult<NodeId> {
    with_page(agent, |agent, page| {
        let Some(document) = page.tree.read() else {
            return agent.type_error("document is busy");
        };
        let Ok(wrappers) = page.wrappers.try_borrow() else {
            return agent.type_error("DOM bindings are busy");
        };
        let id = node_id(agent, &wrappers, &document, receiver)?;
        if !document.node(id).is_ok_and(|node| brand(node.kind())) {
            return agent.type_error(interface);
        }
        Ok(id)
    })
}

fn with_mut_tree<T>(
    agent: &mut Agent,
    operation: impl FnOnce(&mut Agent, &mut Document, &mut DomHeap) -> JsResult<T>,
) -> JsResult<T> {
    with_page(agent, |agent, page| {
        let Some(mut document) = page.tree.write() else {
            return agent.type_error("document is busy");
        };
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        operation(agent, &mut document, &mut wrappers)
    })
}

fn create_node(
    agent: &mut Agent,
    custom_prototype: Option<ObjectId>,
    create: impl FnOnce(&mut Document) -> Result<NodeId, DomError>,
) -> JsResult<Value> {
    with_mut_tree(agent, |agent, document, wrappers| {
        let id = create(document).map_err(|error| dom_exception::from_dom_error(agent, error))?;
        let result = match custom_prototype {
            Some(proto) => wrap_using(wrappers, agent, document, id, proto),
            None => wrap(wrappers, agent, document, id),
        };
        if result.is_err() {
            document
                .destroy_subtree(id)
                .expect("detached node from factory");
        }
        result
    })
}

fn required_arg(agent: &mut Agent, args: &[Value], at: usize) -> JsResult<Value> {
    match args.get(at) {
        Some(&value) => Ok(value),
        None => agent.type_error("required Web IDL argument is missing"),
    }
}

fn read_node<T>(
    agent: &mut Agent,
    receiver: Value,
    read: impl FnOnce(&mut Agent, &mut DomHeap, &Document, NodeId, &Node) -> JsResult<T>,
) -> JsResult<T> {
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
        read(agent, &mut wrappers, &document, id, node)
    })
}

fn node_type(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |_, _, _, _, node| {
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
        |agent, wrappers, document, _, node| match relation(document, node) {
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
    read_node(agent, receiver, |agent, wrappers, document, _, node| {
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
    read_node(agent, receiver, |agent, _, _, _, node| match node.kind() {
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
    edit_child(agent, receiver, args, |document, parent, child| {
        document
            .append_child(parent, child)
            .map_err(|error| match error {
                DomError::ProtectedRoot => DomError::InvalidHierarchy,
                error => error,
            })
    })
}

fn remove_child(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    edit_child(agent, receiver, args, |document, parent, child| {
        if document.node(child)?.parent() != Some(parent) {
            return Err(DomError::InvalidReference);
        }
        document.detach(child)
    })
}
