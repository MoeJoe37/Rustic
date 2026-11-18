//! Detached documents, DOMImplementation and cross-document node operations.

use std::cell::RefCell;

use rustic_dom::{
    Document, DocumentFormat, DocumentLimits, DocumentType, DomError, DomString, HTML_NAMESPACE,
    NodeId, NodeKind, QualifiedName, SVG_NAMESPACE,
};
use rustic_js::runtime::{
    Abrupt, Agent, JsResult, NativeFn, ObjectId, OutOfMemory, PropertyDescriptor, Value,
};

use crate::dom_exception;
use crate::heap::{DomGcError, WrapperKind};
use crate::webidl::to_nullable_dom_string;

use super::creation::{document_receiver, element, exception, invalid_name, qualified, string};
use super::{
    AuxiliaryDocument, document_element_id, finish_creation, identity, illegal_constructor,
    insert_node, install_interface, prototype, quirks_mode, read_node, required_arg, with_mut_tree,
    with_page, wrap, wrap_using,
};

const IMPLEMENTATION: &str = "DOMImplementation";
const MAX_AUXILIARY_DOCUMENTS: usize = 16;

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        "XMLDocument",
        Some("Document"),
        illegal_constructor,
        0,
        &[],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        IMPLEMENTATION,
        None,
        illegal_constructor,
        0,
        &[],
        |agent, prototype| {
            for (name, operation, length) in [
                ("createDocumentType", create_document_type as NativeFn, 3),
                ("createDocument", create_document, 2),
                ("createHTMLDocument", create_html_document, 0),
                ("hasFeature", has_feature, 0),
            ] {
                let key = agent.heap.key(name)?;
                agent.define_method(prototype, key, operation, name, length)?;
            }
            Ok(())
        },
    )
}

pub(super) fn install_methods(agent: &mut Agent, prototype: ObjectId) -> JsResult<()> {
    for (name, operation, length) in [
        ("adoptNode", adopt_node as NativeFn, 1),
        ("createEvent", create_event, 1),
        ("importNode", import_node, 1),
        ("write", write_document, 0),
        ("writeln", write_document, 0),
    ] {
        let key = agent.heap.key(name)?;
        agent.define_method(prototype, key, operation, name, length)?;
    }
    Ok(())
}

pub(super) fn install_body(agent: &mut Agent, prototype: ObjectId) -> JsResult<()> {
    let key = agent.heap.key("body")?;
    let get = Value::object(agent.new_native_function(body, "get body", 0)?);
    let set = Value::object(agent.new_native_function(set_body, "set body", 1)?);
    agent.define_property_or_throw(
        prototype,
        key,
        PropertyDescriptor::accessor(get, set, true, true),
    )
}

fn html_element_name(document: &Document, id: NodeId) -> Option<&str> {
    let NodeKind::Element(element) = document.node(id).ok()?.kind() else {
        return None;
    };
    (element.name().namespace() == Some(HTML_NAMESPACE)).then_some(element.name().local())
}

fn body_child(agent: &mut Agent, document: &Document, root: NodeId) -> JsResult<Option<NodeId>> {
    if html_element_name(document, root) != Some("html") {
        return Ok(None);
    }
    for child in document
        .children(root)
        .map_err(|error| dom_exception::from_dom_error(agent, error))?
    {
        agent.check_native_work()?;
        if matches!(
            html_element_name(document, child),
            Some("body" | "frameset")
        ) {
            return Ok(Some(child));
        }
    }
    Ok(None)
}

fn body(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, wrappers, document, id, _| {
        if id != document.root() {
            return agent.type_error("expected a Document receiver");
        }
        let Some(root) = document_element_id(agent, document)? else {
            return Ok(Value::NULL);
        };
        match body_child(agent, document, root)? {
            Some(node) => wrap(wrappers, agent, document, node),
            None => Ok(Value::NULL),
        }
    })
}

fn set_body(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    with_mut_tree(agent, receiver, |agent, target, wrappers, id| {
        if id != target.root() {
            return agent.type_error("expected a Document receiver");
        }
        let hierarchy = |agent: &mut Agent| {
            exception(
                agent,
                "HierarchyRequestError",
                "expected a body or frameset",
            )
        };
        if value.is_nullish() {
            return Err(hierarchy(agent));
        }
        let node = identity(agent, wrappers, value, WrapperKind::Node)?;
        let check_node =
            |agent: &mut Agent, document: &Document| match html_element_name(document, node) {
                Some("body" | "frameset") => Ok(()),
                Some(_) => Err(hierarchy(agent)),
                None => agent.type_error("expected an HTMLElement"),
            };
        if node.document() == target.id() {
            check_node(agent, target)?;
        } else {
            with_page(agent, |agent, page| {
                page.read(agent, node.document(), check_node)
            })?;
        }
        let Some(root) = document_element_id(agent, target)? else {
            return Err(hierarchy(agent));
        };
        let previous = body_child(agent, target, root)?;
        if previous == Some(node) {
            return Ok(Value::UNDEFINED);
        }
        insert_node(agent, target, wrappers, node, root, previous)?;
        if let Some(previous) = previous {
            target.detach(previous).expect("body is an ordinary child");
        }
        Ok(Value::UNDEFINED)
    })
}

fn create_event(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    let value = required_arg(agent, args, 0)?;
    let name = string(agent, value)?;
    let mut name = String::from_utf16_lossy(name.as_units());
    name.make_ascii_lowercase();
    super::events::create_legacy_event(agent, &name)
}

fn write_document(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, document, _, node| {
        if !matches!(node.kind(), NodeKind::Document) {
            return agent.type_error("expected a Document receiver");
        }
        let name = if document.is_html() {
            "NotSupportedError"
        } else {
            "InvalidStateError"
        };
        Err(exception(
            agent,
            name,
            "dynamic document markup is unavailable",
        ))
    })
}

pub(super) fn current_script(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    let owner = document_receiver(agent, receiver)?;
    with_page(agent, |agent, page| {
        let Some(node) = page
            .tree
            .current_script()
            .filter(|_| owner.document() == page.main)
        else {
            return Ok(Value::NULL);
        };
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        page.read(agent, page.main, |agent, document| {
            wrap(&mut wrappers, agent, document, node)
        })
    })
}

pub(super) fn implementation(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, wrappers, document, id, node| {
        if !matches!(node.kind(), NodeKind::Document) {
            return agent.type_error("expected a Document receiver");
        }
        let proto = prototype(agent, IMPLEMENTATION)?;
        wrappers
            .wrap_with_kind(agent, document, id, proto, WrapperKind::Implementation)
            .map(Value::object)
            .map_err(|error| binding_error(agent, error))
    })
}

fn implementation_id(agent: &mut Agent, receiver: Value) -> JsResult<NodeId> {
    with_page(agent, |agent, page| {
        let Ok(wrappers) = page.wrappers.try_borrow() else {
            return agent.type_error("DOM bindings are busy");
        };
        identity(agent, &wrappers, receiver, WrapperKind::Implementation)
    })
}

pub(super) fn content_type(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, _, document, _, node| {
        if !matches!(node.kind(), NodeKind::Document) {
            return agent.type_error("expected a Document receiver");
        }
        agent.string(document.format().content_type())
    })
}

pub(super) fn ready_state(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    read_node(agent, receiver, |agent, _, document, _, _| {
        let state = match document.ready_state() {
            rustic_dom::ReadyState::Loading => "loading",
            rustic_dom::ReadyState::Interactive => "interactive",
            rustic_dom::ReadyState::Complete => "complete",
        };
        agent.string(state)
    })
}

fn is_hidden(agent: &mut Agent, receiver: Value) -> JsResult<bool> {
    let owner = document_receiver(agent, receiver)?;
    with_page(agent, |_, page| Ok(owner.document() != page.main))
}

pub(super) fn hidden(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(is_hidden(agent, receiver)?))
}

pub(super) fn visibility_state(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    let state = if is_hidden(agent, receiver)? {
        "hidden"
    } else {
        "visible"
    };
    agent.string(state)
}

pub(super) fn doctype(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    read_node(agent, receiver, |agent, wrappers, document, id, node| {
        if !matches!(node.kind(), NodeKind::Document) {
            return agent.type_error("expected a Document receiver");
        }
        for child in document
            .children(id)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?
        {
            agent.check_native_work()?;
            if matches!(
                document.node(child).map(|node| node.kind()),
                Ok(NodeKind::DocumentType(_))
            ) {
                return wrap(wrappers, agent, document, child);
            }
        }
        Ok(Value::NULL)
    })
}

fn main_limits(agent: &mut Agent) -> JsResult<DocumentLimits> {
    with_page(agent, |agent, page| {
        page.read(agent, page.main, |_, document| Ok(document.limits()))
    })
}

fn new_document(agent: &mut Agent, format: DocumentFormat) -> JsResult<Document> {
    let limits = main_limits(agent)?;
    Document::new_with_format(limits, format)
        .map_err(|error| dom_exception::from_dom_error(agent, error))
}

pub(super) fn construct_document(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("Document requires new");
    };
    let prototype = agent.host_prototype_from_new_target("Document", target)?;
    let document = new_document(agent, DocumentFormat::Xml)?;
    register(agent, document, Some(prototype), false, None)
}

fn has_feature(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    implementation_id(agent, receiver)?;
    Ok(Value::TRUE)
}

fn create_document_type(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let owner = implementation_id(agent, receiver)?;
    let name = required_arg(agent, args, 0)?;
    let public_id = required_arg(agent, args, 1)?;
    let system_id = required_arg(agent, args, 2)?;
    let name = string(agent, name)?;
    let public_id = string(agent, public_id)?;
    let system_id = string(agent, system_id)?;
    if name
        .as_units()
        .iter()
        .any(|c| matches!(*c, 0 | 9 | 10 | 12 | 13 | 32 | 62))
    {
        return Err(invalid_name(agent));
    }
    let value = DocumentType {
        name,
        public_id,
        system_id,
    };
    with_page(agent, |agent, page| {
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        page.write(agent, owner.document(), |agent, document| {
            let id = document
                .create_doctype(value)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
            finish_creation(agent, document, &mut wrappers, id, None)
        })
    })
}

fn create_document(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    implementation_id(agent, receiver)?;
    let namespace = required_arg(agent, args, 0)?;
    let qualified_name = required_arg(agent, args, 1)?;
    let namespace = to_nullable_dom_string(agent, namespace)?;
    let name = string(agent, qualified_name)?;
    let doctype = args.get(2).copied().filter(|value| !value.is_nullish());
    let doctype = doctype
        .map(|value| require_doctype(agent, value))
        .transpose()?;
    let name = if name.as_units().is_empty() {
        None
    } else {
        Some(qualified(agent, namespace.as_ref(), &name, false)?)
    };
    let format = match namespace.as_ref() {
        Some(ns) if units_equal(ns, HTML_NAMESPACE) => DocumentFormat::XhtmlDocument,
        Some(ns) if units_equal(ns, SVG_NAMESPACE) => DocumentFormat::SvgDocument,
        _ => DocumentFormat::XmlDocument,
    };
    let mut document = new_document(agent, format)?;
    let root = document.root();
    let create_root = (|| -> Result<Option<NodeId>, DomError> {
        let Some(name) = name else { return Ok(None) };
        let child = element(&mut document, name)?;
        document.append_child(root, child)?;
        Ok(Some(child))
    })();
    let child = create_root.map_err(|error| dom_exception::from_dom_error(agent, error))?;
    register(
        agent,
        document,
        None,
        false,
        doctype.map(|doctype| (doctype, child)),
    )
}

fn require_doctype(agent: &mut Agent, value: Value) -> JsResult<NodeId> {
    super::require_receiver(
        agent,
        value,
        |kind| matches!(kind, NodeKind::DocumentType(_)),
        "expected a DocumentType argument",
    )
}

fn units_equal(value: &DomString, literal: &str) -> bool {
    value.as_units().iter().copied().eq(literal.encode_utf16())
}

fn create_html_document(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    implementation_id(agent, receiver)?;
    let title = args
        .first()
        .copied()
        .filter(|&value| value != Value::UNDEFINED)
        .map(|value| string(agent, value))
        .transpose()?;
    let mut document = new_document(agent, DocumentFormat::Html)?;
    html_skeleton(&mut document, title)
        .map_err(|error| dom_exception::from_dom_error(agent, error))?;
    register(agent, document, None, false, None)
}

fn html_skeleton(document: &mut Document, title: Option<DomString>) -> Result<(), DomError> {
    let root = document.root();
    let doctype = document.create_doctype(DocumentType {
        name: DomString::from("html"),
        public_id: DomString::default(),
        system_id: DomString::default(),
    })?;
    document.append_child(root, doctype)?;
    let html = append_element(document, root, "html")?;
    let head = append_element(document, html, "head")?;
    if let Some(title) = title {
        let element = append_element(document, head, "title")?;
        let text = document.create_text(title)?;
        document.append_child(element, text)?;
    }
    append_element(document, html, "body")?;
    Ok(())
}

fn append_element(document: &mut Document, parent: NodeId, name: &str) -> Result<NodeId, DomError> {
    let node = document.create_element(QualifiedName::html(name)?)?;
    document.append_child(parent, node)?;
    Ok(node)
}

fn register(
    agent: &mut Agent,
    mut document: Document,
    custom_prototype: Option<ObjectId>,
    quirks: bool,
    doctype: Option<(NodeId, Option<NodeId>)>,
) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        {
            let Ok(mut documents) = page.additional.try_borrow_mut() else {
                return agent.type_error("document registry is busy");
            };
            if documents.len() >= MAX_AUXILIARY_DOCUMENTS {
                return Err(exception(
                    agent,
                    "QuotaExceededError",
                    "too many detached documents",
                ));
            }
            documents.try_reserve(1).map_err(|_| OutOfMemory)?;
        }
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        wrappers
            .register_document(&document, false)
            .map_err(|error| binding_error(agent, error))?;
        let root = document.root();
        let result = (|| {
            let value = match custom_prototype {
                Some(prototype) => wrap_using(&mut wrappers, agent, &document, root, prototype)?,
                None => wrap(&mut wrappers, agent, &document, root)?,
            };
            if let Some((node, before)) = doctype {
                page.write_adoption_source(agent, node, |agent, source| {
                    charge_copy(agent, source, node, false, false)?;
                    wrappers
                        .insert_adopted(&mut document, source, node, root, before)
                        .map_err(|error| binding_error(agent, error))?;
                    Ok(())
                })?;
            }
            Ok(value)
        })();
        if result.is_ok() {
            // No script runs from reservation through publication.
            page.additional.borrow_mut().insert(
                document.id(),
                AuxiliaryDocument {
                    tree: RefCell::new(document),
                    quirks,
                },
            );
        } else {
            wrappers.discard_document(document.id());
        }
        result
    })
}

fn adopt_node(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    let value = required_arg(agent, args, 0)?;
    with_mut_tree(agent, receiver, |agent, target, wrappers, root| {
        let node = identity(agent, wrappers, value, WrapperKind::Node)?;
        if node.document() == root.document() {
            target
                .detach(node)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        } else {
            with_page(agent, |agent, page| {
                page.write_adoption_source(agent, node, |agent, source| {
                    charge_copy(agent, source, node, true, true)?;
                    wrappers
                        .adopt_node(target, source, node)
                        .map_err(|error| binding_error(agent, error))?;
                    Ok(())
                })
            })?;
        }
        Ok(value)
    })
}

fn import_node(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    document_receiver(agent, receiver)?;
    let value = required_arg(agent, args, 0)?;
    read_node(agent, value, |_, _, _, _, _| Ok(()))?;
    let deep = import_subtree(agent, args.get(1).copied())?;
    with_mut_tree(agent, receiver, |agent, target, wrappers, root| {
        let node = identity(agent, wrappers, value, WrapperKind::Node)?;
        let copy = if node.document() == root.document() {
            charge_clone(agent, target, node, deep)?;
            target.clone_node(node, deep)
        } else {
            let id = with_page(agent, |agent, page| {
                page.read(agent, node.document(), |agent, source| {
                    charge_clone(agent, source, node, deep)?;
                    target
                        .import_node(source, node, deep)
                        .map_err(|error| dom_exception::from_dom_error(agent, error))
                })
            })?;
            Ok(id)
        }
        .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        finish_creation(agent, target, wrappers, copy, None)
    })
}

fn import_subtree(agent: &mut Agent, option: Option<Value>) -> JsResult<bool> {
    let Some(option) = option.filter(|&value| value != Value::UNDEFINED) else {
        return Ok(false);
    };
    if option == Value::NULL {
        return Ok(true);
    }
    if let Some(object) = option.as_object() {
        let key = agent.heap.key("customElementRegistry")?;
        if agent.get(object, key)? != Value::UNDEFINED {
            return agent
                .type_error("customElementRegistry must be a supported CustomElementRegistry");
        }
        let key = agent.heap.key("selfOnly")?;
        let value = agent.get(object, key)?;
        return Ok(!agent.to_boolean(value));
    }
    Ok(agent.to_boolean(option))
}

fn charge_clone(agent: &mut Agent, document: &Document, node: NodeId, deep: bool) -> JsResult<()> {
    let source = document
        .node(node)
        .map_err(|error| dom_exception::from_dom_error(agent, error))?;
    if node == document.root() || source.shadow_host().is_some() {
        return Err(exception(
            agent,
            "NotSupportedError",
            "cannot import this node",
        ));
    }
    charge_copy(agent, document, node, deep, false)
}

pub(super) fn clone_node(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let deep = args.first().is_some_and(|&value| agent.to_boolean(value));
    let copy = read_node(agent, receiver, |agent, _, document, id, node| {
        if !matches!(node.kind(), NodeKind::Document) {
            return Ok(None);
        }
        charge_copy(agent, document, id, deep, false)?;
        let quirks = quirks_mode(agent, id)?;
        let copy = document
            .clone_document_with_children(deep)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        Ok(Some((copy, quirks)))
    })?;
    if let Some((copy, quirks)) = copy {
        return register(agent, copy, None, quirks, None);
    }
    with_mut_tree(agent, receiver, |agent, document, wrappers, node| {
        charge_clone(agent, document, node, deep)?;
        let copy = document
            .clone_node(node, deep)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        finish_creation(agent, document, wrappers, copy, None)
    })
}

pub(super) fn charge_copy(
    agent: &mut Agent,
    document: &Document,
    root: NodeId,
    deep: bool,
    shadow: bool,
) -> JsResult<()> {
    let mut owned = Vec::new();
    let mut next = Some(root);
    while let Some(root) = next {
        let nodes = document
            .walk(root)
            .map_err(|error| dom_exception::from_dom_error(agent, error))?;
        for (id, _) in nodes.take(if deep { usize::MAX } else { 1 }) {
            agent.check_native_work()?;
            let node = document
                .node(id)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
            for _ in (0..node.retained_data_bytes()).step_by(64) {
                agent.check_native_work()?;
            }
            if deep {
                if let NodeKind::Element(element) = node.kind() {
                    let roots = element
                        .template_contents()
                        .into_iter()
                        .chain(element.shadow_root().filter(|_| shadow));
                    owned
                        .try_reserve(roots.size_hint().1.unwrap_or(0))
                        .map_err(|_| OutOfMemory)?;
                    owned.extend(roots);
                }
            }
        }
        next = owned.pop();
    }
    Ok(())
}

pub(super) fn binding_error(agent: &mut Agent, error: DomGcError) -> Abrupt {
    match error {
        DomGcError::Dom(error) => dom_exception::from_dom_error(agent, error),
        _ => agent.throw(
            rustic_js::runtime::ErrorKind::TypeError,
            "node is outside this document host",
        ),
    }
}
