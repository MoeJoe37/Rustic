//! The document host and the first live DOM bindings for page scripts.

use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::fmt::Debug;
use std::rc::Rc;

use rustic_dom::{Document, DocumentFormat, DocumentId, DomError, Node, NodeId, NodeKind};
use rustic_js::runtime::{
    Abrupt, Agent, ExecutionHost, JsResult, NativeFn, ObjectId, OutOfMemory, PropertyDescriptor,
    PropertyKey, Roots, Value,
};

use crate::heap::WrapperKind;
use crate::{DomGcError, DomHeap, dom_exception};

mod attributes;
mod character_data;
mod collections;
mod computed_style;
mod creation;
mod documents;
mod dom_rect;
mod events;
mod html_collections;
mod mutation_observer;
mod node_tree;
mod selectors;
mod window;

pub use events::LifecycleEvent;
pub use events::{
    CONSOLE_CAPACITY, CONSOLE_MESSAGE_BYTES, ConsoleLevel, ConsoleLog, ConsoleMessage,
};
pub use events::{InputEvent, KeyInput, Modifiers};
pub use window::{TimerHandler, WindowScheduler};

/// A rendered layout read could not produce a result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutReadError {
    /// The current turn has spent its layout flush budget.
    Limit,
    /// The host could not compute a bounded layout snapshot.
    Unavailable,
}

/// Owned CSSOM longhand and custom-property values at one layout revision.
///
/// No JavaScript object owns this record: a held CSSStyleDeclaration reads a
/// fresh record through the shared revision-keyed layout cache on each access.
#[derive(Clone, Debug, Default)]
pub struct ComputedStyleSnapshot {
    /// Supported CSS longhands in canonical property-name order.
    pub longhands: Vec<(&'static str, String)>,
    /// Computed custom properties in name order (case-sensitive).
    pub custom_properties: Vec<(String, String)>,
}

impl ComputedStyleSnapshot {
    fn get(&self, name: &str) -> Option<&str> {
        if name.starts_with("--") {
            return self
                .custom_properties
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str());
        }
        self.get_longhand(rustic_css::PropertyId::from_name(name)?)
    }

    fn get_longhand(&self, property: rustic_css::PropertyId) -> Option<&str> {
        self.longhands
            .iter()
            .find(|(key, _)| *key == property.name())
            .map(|(_, value)| value.as_str())
    }

    fn name_at(&self, index: usize) -> Option<&str> {
        self.longhands
            .get(index)
            .map(|(name, _)| *name)
            .or_else(|| {
                index
                    .checked_sub(self.longhands.len())
                    .and_then(|offset| self.custom_properties.get(offset))
                    .map(|(name, _)| name.as_str())
            })
    }

    fn len(&self) -> usize {
        self.longhands.len() + self.custom_properties.len()
    }
}

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
    /// The canonical name of the decoder used for the main document.
    fn encoding_name(&self) -> &'static str {
        "UTF-8"
    }
    /// Nodes retained by the host independently of their JavaScript wrappers.
    fn retained_nodes(&self) -> Result<Vec<NodeId>, OutOfMemory> {
        Ok(Vec::new())
    }
    /// The classic script currently executing, if any.
    fn current_script(&self) -> Option<NodeId> {
        None
    }
    /// The untransformed rounded border-fragment width in CSS pixels.
    fn offset_width(&self, _node: NodeId) -> Result<i32, LayoutReadError> {
        Ok(0)
    }
    /// The transformed border-fragment bounds as `[x, y, width, height]`.
    fn client_rect(&self, _node: NodeId) -> Result<[f64; 4], LayoutReadError> {
        Ok([0.0; 4])
    }
    /// Compute the supported CSS properties from the same layout inputs as geometry.
    fn computed_style(&self, _node: NodeId) -> Result<ComputedStyleSnapshot, LayoutReadError> {
        Err(LayoutReadError::Unavailable)
    }
    /// Whether a subtree may move out of the main document, within the turn budget.
    fn allows_adoption_from_main(&self, _agent: &mut Agent, _node: NodeId) -> JsResult<bool> {
        Ok(true)
    }
}

#[derive(Debug)]
struct AuxiliaryDocument {
    tree: RefCell<Document>,
    quirks: bool,
}

#[derive(Debug)]
struct Page {
    tree: Rc<dyn PageTree>,
    main: DocumentId,
    additional: RefCell<HashMap<DocumentId, AuxiliaryDocument>>,
    root: Value,
    window: Value,
    scheduler: Option<Rc<dyn WindowScheduler>>,
    events: events::Events,
    rects: ObjectId,
    computed_styles: ObjectId,
    wrappers: RefCell<DomHeap>,
    child_lists: RefCell<HashMap<(ObjectId, collections::ChildKind), ObjectId>>,
    child_index: RefCell<collections::ChildIndex>,
    observers: mutation_observer::Observers,
}

impl ExecutionHost for Page {
    fn collect(&self, agent: &mut Agent) -> Result<(), OutOfMemory> {
        let native_nodes = self.tree.retained_nodes()?;
        let mut document = self.tree.write().ok_or(OutOfMemory)?;
        let mut wrappers = self.wrappers.try_borrow_mut().map_err(|_| OutOfMemory)?;
        let mut additional = self.additional.try_borrow_mut().map_err(|_| OutOfMemory)?;
        let mut trees = Vec::new();
        trees
            .try_reserve_exact(additional.len().saturating_add(1))
            .map_err(|_| OutOfMemory)?;
        trees.push(&mut *document);
        for cell in additional.values_mut() {
            trees.push(cell.tree.get_mut());
        }
        wrappers
            .collect_documents_with_nodes(agent, &mut trees, Roots::default(), &native_nodes)
            .map_err(|_| OutOfMemory)?;
        additional.retain(|&id, _| wrappers.has_document(id));
        self.child_lists
            .try_borrow_mut()
            .map_err(|_| OutOfMemory)?
            .retain(|(object, _), _| {
                wrappers
                    .identity(agent, *object, WrapperKind::Node)
                    .is_some()
            });
        self.child_index
            .try_borrow_mut()
            .map_err(|_| OutOfMemory)?
            .discard_collected(|id| {
                if id.document() == self.main {
                    document.node(id).is_ok()
                } else {
                    additional
                        .get_mut(&id.document())
                        .is_some_and(|cell| cell.tree.get_mut().node(id).is_ok())
                }
            });
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Page {
    fn write_adoption_source<T>(
        &self,
        agent: &mut Agent,
        node: NodeId,
        operation: impl FnOnce(&mut Agent, &mut Document) -> JsResult<T>,
    ) -> JsResult<T> {
        let id = node.document();
        if id == self.main && !self.tree.allows_adoption_from_main(agent, node)? {
            return Err(creation::exception(
                agent,
                "NotSupportedError",
                "cannot adopt a subtree retained by the parser or a pending script",
            ));
        }
        self.write(agent, id, operation)
    }

    fn read<T>(
        &self,
        agent: &mut Agent,
        id: DocumentId,
        operation: impl FnOnce(&mut Agent, &Document) -> JsResult<T>,
    ) -> JsResult<T> {
        if id == self.main {
            let Some(document) = self.tree.read() else {
                return agent.type_error("document is busy");
            };
            return operation(agent, &document);
        }
        let Ok(documents) = self.additional.try_borrow() else {
            return agent.type_error("document registry is busy");
        };
        let Some(cell) = documents.get(&id) else {
            return agent.type_error("document is outside this page");
        };
        let Ok(document) = cell.tree.try_borrow() else {
            return agent.type_error("document is busy");
        };
        operation(agent, &document)
    }

    fn write<T>(
        &self,
        agent: &mut Agent,
        id: DocumentId,
        operation: impl FnOnce(&mut Agent, &mut Document) -> JsResult<T>,
    ) -> JsResult<T> {
        if id == self.main {
            let Some(mut document) = self.tree.write() else {
                return agent.type_error("document is busy");
            };
            return operation(agent, &mut document);
        }
        let Ok(documents) = self.additional.try_borrow() else {
            return agent.type_error("document registry is busy");
        };
        let Some(cell) = documents.get(&id) else {
            return agent.type_error("document is outside this page");
        };
        let Ok(mut document) = cell.tree.try_borrow_mut() else {
            return agent.type_error("document is busy");
        };
        operation(agent, &mut document)
    }
}

fn quirks_mode(agent: &mut Agent, node: NodeId) -> JsResult<bool> {
    with_page(agent, |agent, page| {
        if node.document() == page.main {
            return Ok(page.tree.quirks_mode());
        }
        let Ok(documents) = page.additional.try_borrow() else {
            return agent.type_error("document registry is busy");
        };
        Ok(documents
            .get(&node.document())
            .is_some_and(|document| document.quirks))
    })
}

/// Attach joint collection and install the root of a host's document in the
/// running realm. The host installs [`dom_exception`] before this entry point.
pub fn install(agent: &mut Agent, tree: Rc<dyn PageTree>) -> JsResult<()> {
    install_page(agent, tree, None)
}

/// Install the page with Window timers and animation frame callbacks.
pub fn install_with_scheduler(
    agent: &mut Agent,
    tree: Rc<dyn PageTree>,
    scheduler: Rc<dyn WindowScheduler>,
) -> JsResult<()> {
    install_page(agent, tree, Some(scheduler))
}

fn install_page(
    agent: &mut Agent,
    tree: Rc<dyn PageTree>,
    scheduler: Option<Rc<dyn WindowScheduler>>,
) -> JsResult<()> {
    if agent.execution_host().is_some() {
        return agent.type_error("an execution host is already attached");
    }
    let Some(document) = tree.read() else {
        return agent.type_error("document is busy");
    };
    events::install(agent)?;
    dom_rect::install(agent)?;
    computed_style::install(agent)?;
    collections::install(agent)?;
    html_collections::install(agent)?;
    mutation_observer::install(agent)?;
    install_interface(
        agent,
        "Node",
        Some(events::TARGET),
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
            character_data::install_text_content(agent, prototype)?;
            node_tree::install(agent, prototype)?;
            for (name, behavior, length) in [
                ("appendChild", append_child as NativeFn, 1),
                ("insertBefore", insert_before, 2),
                ("removeChild", remove_child, 1),
                ("cloneNode", documents::clone_node, 0),
            ] {
                let key = agent.heap.key(name)?;
                agent.define_method(prototype, key, behavior, name, length)?;
            }
            Ok(())
        },
    )?;
    install_node_constants(agent)?;
    install_interface(
        agent,
        "Document",
        Some("Node"),
        documents::construct_document,
        0,
        &[
            ("documentElement", "get documentElement", document_element),
            ("defaultView", "get defaultView", window::default_view),
            (
                "implementation",
                "get implementation",
                documents::implementation,
            ),
            ("contentType", "get contentType", documents::content_type),
            ("characterSet", "get characterSet", documents::character_set),
            ("charset", "get charset", documents::character_set),
            (
                "inputEncoding",
                "get inputEncoding",
                documents::character_set,
            ),
            ("doctype", "get doctype", documents::doctype),
            ("readyState", "get readyState", documents::ready_state),
            ("hidden", "get hidden", documents::hidden),
            (
                "visibilityState",
                "get visibilityState",
                documents::visibility_state,
            ),
            (
                "currentScript",
                "get currentScript",
                documents::current_script,
            ),
            (
                "children",
                "get children",
                html_collections::document_children,
            ),
        ],
        |agent, proto| {
            creation::install_factories(agent, proto)?;
            documents::install_methods(agent, proto)?;
            documents::install_body(agent, proto)?;
            selectors::install_parent_node(agent, proto)?;
            selectors::install_non_element_parent(agent, proto)?;
            events::install_handlers(agent, proto, false)?;
            html_collections::install_lookup::<true>(agent, proto)
        },
    )?;
    documents::install(agent)?;
    install_interface(
        agent,
        "Element",
        Some("Node"),
        illegal_constructor,
        0,
        &[
            ("localName", "get localName", local_name),
            ("prefix", "get prefix", creation::prefix),
            ("namespaceURI", "get namespaceURI", creation::namespace_uri),
            ("tagName", "get tagName", creation::tag_name),
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
            events::define_method(
                agent,
                proto,
                "getBoundingClientRect",
                bounding_client_rect,
                0,
            )?;
            html_collections::install_lookup::<false>(agent, proto)
        },
    )?;
    install_interface(
        agent,
        "HTMLElement",
        Some("Element"),
        illegal_constructor,
        0,
        &[("offsetWidth", "get offsetWidth", offset_width)],
        |agent, proto| events::install_handlers(agent, proto, false),
    )?;
    events::install_controls(agent)?;
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
    let main = document.id();
    drop(document);
    let window = window::install(agent, scheduler.is_some())?;
    let events = events::Events::new(agent)?;
    let rects = agent.new_weak_map()?;
    agent.root(Value::object(rects))?;
    let computed_styles = agent.new_weak_map()?;
    agent.root(Value::object(computed_styles))?;
    let observers = mutation_observer::Observers::new(agent)?;
    agent.attach_execution_host(Rc::new(Page {
        tree,
        main,
        additional: RefCell::new(HashMap::new()),
        root,
        window,
        scheduler,
        events,
        rects,
        computed_styles,
        wrappers: RefCell::new(wrappers),
        child_lists: RefCell::new(HashMap::new()),
        child_index: RefCell::new(collections::ChildIndex::default()),
        observers,
    }))
}

/// Whether a host input target has an unbroken parent path to this document.
pub fn connected_input_target(document: &Document, target: NodeId) -> bool {
    events::connected(document, target)
}

/// Deliver host input in the active page. A target that went away is skipped.
pub fn deliver_input(agent: &mut Agent, target: NodeId, input: &InputEvent) -> JsResult<()> {
    events::deliver(agent, target, input)
}

/// Deliver a trusted event owned by the scripted document lifecycle.
pub fn deliver_lifecycle(agent: &mut Agent, event: LifecycleEvent) -> JsResult<()> {
    events::deliver_lifecycle(agent, event)
}

/// Queue mutation records for edits a host or parser made outside a binding
/// operation. Call before each microtask checkpoint; a page without
/// observers returns at once.
pub fn flush_mutation_records(agent: &mut Agent) -> JsResult<()> {
    let host = agent.execution_host();
    let Some(page) = host
        .as_ref()
        .and_then(|host| host.as_any().downcast_ref::<Page>())
    else {
        return Ok(());
    };
    mutation_observer::flush(agent, page)
}

/// HTML "report an exception" for a value thrown by a script, timer or
/// callback the host ran: a cancelable `error` ErrorEvent at Window, then a
/// console message unless a handler canceled it.
pub fn report_exception(agent: &mut Agent, error: Value) -> JsResult<()> {
    events::report_exception(agent, error)
}

/// Fire `unhandledrejection` and `rejectionhandled` for the rejection
/// tracker's events, as HTML "notify about rejected promises" does. Call
/// after a microtask checkpoint with [`Agent::take_rejection_events`].
pub fn report_rejections(
    agent: &mut Agent,
    events: Vec<rustic_js::runtime::RejectionEvent>,
) -> JsResult<()> {
    events::report_rejections(agent, events)
}

/// Take the page's console messages, oldest first, and the number dropped
/// because the bounded buffer was full.
pub fn take_console_messages(agent: &Agent) -> ConsoleLog {
    let host = agent.execution_host();
    host.as_ref()
        .and_then(|host| host.as_any().downcast_ref::<Page>())
        .map(|page| page.events.reporting.take_console())
        .unwrap_or_default()
}

/// Read and clear the number of exceptions reported by event listeners.
pub fn take_listener_error_count(agent: &Agent) -> u64 {
    let host = agent.execution_host();
    host.as_ref()
        .and_then(|host| host.as_any().downcast_ref::<Page>())
        .map_or(0, |page| page.events.take_listener_error_count())
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

/// The `Node` type and document-position constants on the interface object
/// and prototype.
fn install_node_constants(agent: &mut Agent) -> JsResult<()> {
    let prototype = prototype(agent, "Node")?;
    let key = PropertyKey::String(agent.names.constructor);
    let Some(constructor) = agent.get(prototype, key)?.as_object() else {
        return agent.type_error("Node constructor is missing");
    };
    for (value, name) in [
        "ELEMENT_NODE",
        "ATTRIBUTE_NODE",
        "TEXT_NODE",
        "CDATA_SECTION_NODE",
        "ENTITY_REFERENCE_NODE",
        "ENTITY_NODE",
        "PROCESSING_INSTRUCTION_NODE",
        "COMMENT_NODE",
        "DOCUMENT_NODE",
        "DOCUMENT_TYPE_NODE",
        "DOCUMENT_FRAGMENT_NODE",
        "NOTATION_NODE",
    ]
    .into_iter()
    .enumerate()
    {
        let key = agent.heap.key(name)?;
        let number = Value::number(f64::from(
            u8::try_from(value + 1).expect("twelve constants"),
        ));
        let desc = PropertyDescriptor::data_with(number, false, true, false);
        agent.define_property_or_throw(prototype, key, desc)?;
        agent.define_property_or_throw(constructor, key, desc)?;
    }
    for (name, bit) in [
        ("DOCUMENT_POSITION_DISCONNECTED", 0x01),
        ("DOCUMENT_POSITION_PRECEDING", 0x02),
        ("DOCUMENT_POSITION_FOLLOWING", 0x04),
        ("DOCUMENT_POSITION_CONTAINS", 0x08),
        ("DOCUMENT_POSITION_CONTAINED_BY", 0x10),
        ("DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC", 0x20),
    ] {
        let key = agent.heap.key(name)?;
        let desc = PropertyDescriptor::data_with(Value::number(f64::from(bit)), false, true, false);
        agent.define_property_or_throw(prototype, key, desc)?;
        agent.define_property_or_throw(constructor, key, desc)?;
    }
    Ok(())
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
        NodeKind::Document => match document.format() {
            DocumentFormat::Html | DocumentFormat::Xml => "Document",
            _ => "XMLDocument",
        },
        NodeKind::Element(element)
            if element.name().namespace() == Some(rustic_dom::HTML_NAMESPACE)
                && element.name().local() == "input" =>
        {
            "HTMLInputElement"
        }
        NodeKind::Element(element)
            if element.name().namespace() == Some(rustic_dom::HTML_NAMESPACE) =>
        {
            "HTMLElement"
        }
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
    let result = operation(agent, page);
    if page.observers.is_active() {
        // Records are queued as soon as the outermost operation releases the
        // page, so interested observers see the tree as it was changed.
        mutation_observer::flush(agent, page)?;
    }
    result
}

fn identity(
    agent: &mut Agent,
    wrappers: &DomHeap,
    value: Value,
    kind: WrapperKind,
) -> JsResult<NodeId> {
    match value
        .as_object()
        .and_then(|object| wrappers.identity(agent, object, kind))
    {
        Some(id) => Ok(id),
        None => agent.type_error("invalid DOM receiver or node argument"),
    }
}

fn require_receiver(
    agent: &mut Agent,
    receiver: Value,
    brand: fn(&NodeKind) -> bool,
    interface: &str,
) -> JsResult<NodeId> {
    read_node(agent, receiver, |agent, _, _, id, node| {
        if brand(node.kind()) {
            Ok(id)
        } else {
            agent.type_error(interface)
        }
    })
}

fn offset_width(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    let width = read_layout(agent, receiver, true, 0, |tree, id| tree.offset_width(id))?;
    Ok(Value::number(f64::from(width)))
}

fn bounding_client_rect(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    let rect = read_layout(agent, receiver, false, [0.0; 4], |tree, id| {
        tree.client_rect(id)
    })?;
    dom_rect::new_mutable(agent, rect)
}

fn read_layout<T>(
    agent: &mut Agent,
    receiver: Value,
    html_only: bool,
    empty: T,
    read: impl FnOnce(&dyn PageTree, NodeId) -> Result<T, LayoutReadError>,
) -> JsResult<T> {
    let element = read_node(agent, receiver, |agent, _, document, id, node| {
        let NodeKind::Element(element) = node.kind() else {
            return agent.type_error("expected Element");
        };
        if html_only && element.name().namespace() != Some(rustic_dom::HTML_NAMESPACE) {
            return agent.type_error("expected HTMLElement");
        }
        events::reaches_root(document, id, || agent.check_native_work())
            .map(|connected| connected.then_some(id))
    })?;
    with_page(agent, |agent, page| {
        let Some(id) = element.filter(|id| id.document() == page.main) else {
            return Ok(empty);
        };
        let value = read(page.tree.as_ref(), id);
        agent.check_native_work()?;
        match value {
            Ok(value) => Ok(value),
            Err(LayoutReadError::Limit) => Err(Abrupt::Throw(dom_exception::new(
                agent,
                "QuotaExceededError",
                "layout flush budget exhausted",
            )?)),
            Err(LayoutReadError::Unavailable) => Err(Abrupt::Throw(dom_exception::new(
                agent,
                "InvalidStateError",
                "layout snapshot unavailable",
            )?)),
        }
    })
}

fn with_mut_tree<T>(
    agent: &mut Agent,
    receiver: Value,
    operation: impl FnOnce(&mut Agent, &mut Document, &mut DomHeap, NodeId) -> JsResult<T>,
) -> JsResult<T> {
    with_page(agent, |agent, page| {
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        let id = identity(agent, &wrappers, receiver, WrapperKind::Node)?;
        page.write(agent, id.document(), |agent, document| {
            document
                .node(id)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
            operation(agent, document, &mut wrappers, id)
        })
    })
}

fn create_node(
    agent: &mut Agent,
    receiver: Option<Value>,
    custom_prototype: Option<ObjectId>,
    create: impl FnOnce(&mut Document) -> Result<NodeId, DomError>,
) -> JsResult<Value> {
    let create = |agent: &mut Agent, document: &mut Document, wrappers: &mut DomHeap| {
        let id = create(document).map_err(|error| dom_exception::from_dom_error(agent, error))?;
        finish_creation(agent, document, wrappers, id, custom_prototype)
    };
    if let Some(receiver) = receiver {
        return with_mut_tree(agent, receiver, |agent, document, wrappers, _| {
            create(agent, document, wrappers)
        });
    }
    with_page(agent, |agent, page| {
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        page.write(agent, page.main, |agent, document| {
            create(agent, document, &mut wrappers)
        })
    })
}

fn finish_creation(
    agent: &mut Agent,
    document: &mut Document,
    wrappers: &mut DomHeap,
    id: NodeId,
    custom_prototype: Option<ObjectId>,
) -> JsResult<Value> {
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
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        let id = identity(agent, &wrappers, receiver, WrapperKind::Node)?;
        page.read(agent, id.document(), |agent, document| {
            let node = document
                .node(id)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
            // These operations never run script while the tree or cache is borrowed.
            read(agent, &mut wrappers, document, id, node)
        })
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
        match document_element_id(agent, document)? {
            Some(id) => wrap(wrappers, agent, document, id),
            None => Ok(Value::NULL),
        }
    })
}

fn document_element_id(agent: &mut Agent, document: &Document) -> JsResult<Option<NodeId>> {
    let children = document
        .children(document.root())
        .map_err(|error| dom_exception::from_dom_error(agent, error))?;
    for id in children {
        agent.check_native_work()?;
        if matches!(document.node(id).map(Node::kind), Ok(NodeKind::Element(_))) {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

fn local_name(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, _, _, node| match node.kind() {
        NodeKind::Element(element) => agent.string(element.name().local()),
        _ => agent.type_error("expected an Element receiver"),
    })
}

fn append_child(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let value = required_arg(agent, args, 0)?;
    insert_child(agent, receiver, value, Value::NULL)
}

fn insert_before(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let value = required_arg(agent, args, 0)?;
    let before = required_arg(agent, args, 1)?;
    insert_child(agent, receiver, value, before)
}

fn insert_child(
    agent: &mut Agent,
    receiver: Value,
    value: Value,
    before: Value,
) -> JsResult<Value> {
    with_mut_tree(agent, receiver, |agent, target, wrappers, parent| {
        let child = identity(agent, wrappers, value, WrapperKind::Node)?;
        let before = if before.is_nullish() {
            None
        } else {
            let node = identity(agent, wrappers, before, WrapperKind::Node)?;
            if node.document() != parent.document() {
                return Err(dom_exception::from_dom_error(
                    agent,
                    DomError::InvalidReference,
                ));
            }
            Some(node)
        };
        insert_node(agent, target, wrappers, child, parent, before)?;
        Ok(value)
    })
}

fn insert_node(
    agent: &mut Agent,
    target: &mut Document,
    wrappers: &mut DomHeap,
    child: NodeId,
    parent: NodeId,
    before: Option<NodeId>,
) -> JsResult<()> {
    if child.document() == parent.document() {
        return target
            .insert_before(parent, child, before)
            .map_err(|error| append_error(agent, error));
    }
    with_page(agent, |agent, page| {
        page.write_adoption_source(agent, child, |agent, source| {
            documents::charge_copy(agent, source, child, true, true)?;
            wrappers
                .insert_adopted(target, source, child, parent, before)
                .map_err(|error| match error {
                    DomGcError::Dom(error) => append_error(agent, error),
                    error => documents::binding_error(agent, error),
                })?;
            Ok(())
        })
    })
}

fn append_error(agent: &mut Agent, error: DomError) -> rustic_js::runtime::Abrupt {
    dom_exception::from_dom_error(
        agent,
        match error {
            DomError::ProtectedRoot => DomError::InvalidHierarchy,
            error => error,
        },
    )
}

fn remove_child(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let value = required_arg(agent, args, 0)?;
    with_mut_tree(agent, receiver, |agent, document, wrappers, parent| {
        let child = identity(agent, wrappers, value, WrapperKind::Node)?;
        if child.document() != parent.document()
            || document.node(child).ok().and_then(Node::parent) != Some(parent)
        {
            return Err(dom_exception::from_dom_error(
                agent,
                DomError::InvalidReference,
            ));
        }
        document
            .detach(child)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        Ok(value)
    })
}
