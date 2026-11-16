//! Live HTMLCollection views of children and filtered descendants.

use std::{collections::HashSet, ops::ControlFlow};

use rustic_dom::{Document, Element, HTML_NAMESPACE, NodeId, NodeKind};
use rustic_js::runtime::{
    Agent, Class, JsResult, LiveIndexAccess, LiveNameAccess, NativeFn, ObjectId, OutOfMemory,
    PropertyDescriptor, PropertyKey, StringId, Value,
};

use crate::dom_exception;
use crate::webidl::{IntegerConversion, coerce_string, from_dom_units, to_unsigned_long};

use super::{
    DomHeap, collections, illegal_constructor, install_interface, prototype, read_node,
    require_receiver, required_arg, with_page, wrap,
};

const HTML_COLLECTION: &str = "HTMLCollection";

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        HTML_COLLECTION,
        None,
        illegal_constructor,
        0,
        &[("length", "get length", length)],
        |agent, prototype| {
            let key = agent.heap.key("item")?;
            agent.define_method(prototype, key, item, "item", 1)?;
            let key = agent.heap.key("namedItem")?;
            agent.define_method(prototype, key, named_item, "namedItem", 1)?;
            agent.define_property_or_throw(
                prototype,
                PropertyKey::Symbol(agent.symbols.iterator),
                PropertyDescriptor::data_with(
                    Value::object(agent.intrinsics.array_values),
                    true,
                    false,
                    true,
                ),
            )
        },
    )
}

pub(super) fn install_lookup<const DOCUMENT: bool>(
    agent: &mut Agent,
    prototype: ObjectId,
) -> JsResult<()> {
    let methods: [(&str, NativeFn, u32); 3] = [
        ("getElementsByTagName", by_tag::<DOCUMENT>, 1),
        ("getElementsByTagNameNS", by_namespace::<DOCUMENT>, 2),
        ("getElementsByClassName", by_class::<DOCUMENT>, 1),
    ];
    for (name, call, arity) in methods {
        let key = agent.heap.key(name)?;
        agent.define_method(prototype, key, call, name, arity)?;
    }
    Ok(())
}

static ACCESS: LiveIndexAccess = LiveIndexAccess {
    length: live_length,
    item: live_item,
    named: Some(&NAMES),
};
static NAMES: LiveNameAccess = LiveNameAccess {
    contains: contains_name,
    item: live_named_item,
    keys: name_keys,
};

pub(super) fn document_children(agent: &mut Agent, source: Value, _: &[Value]) -> JsResult<Value> {
    children(agent, source, |kind| matches!(kind, NodeKind::Document))
}

pub(super) fn element_children(agent: &mut Agent, source: Value, _: &[Value]) -> JsResult<Value> {
    children(agent, source, |kind| matches!(kind, NodeKind::Element(_)))
}

pub(super) fn fragment_children(agent: &mut Agent, source: Value, _: &[Value]) -> JsResult<Value> {
    children(agent, source, |kind| {
        matches!(kind, NodeKind::DocumentFragment)
    })
}

fn children(agent: &mut Agent, source: Value, brand: fn(&NodeKind) -> bool) -> JsResult<Value> {
    require_receiver(agent, source, brand, "invalid ParentNode receiver")?;
    collections::cached_children(
        agent,
        source,
        collections::ChildKind::Elements,
        HTML_COLLECTION,
        &ACCESS,
    )
}

fn length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let object = collections::receiver(agent, this, HTML_COLLECTION)?;
    Ok(Value::number(f64::from(agent.indexed_host_len(object)?)))
}

fn item(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let object = collections::receiver(agent, this, HTML_COLLECTION)?;
    let index = required_arg(agent, args, 0)?;
    let index = to_unsigned_long(agent, index, IntegerConversion::Wrap)?;
    Ok(agent
        .indexed_host_item(object, index)?
        .unwrap_or(Value::NULL))
}

fn named_item(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let object = collections::receiver(agent, this, HTML_COLLECTION)?;
    let name = required_arg(agent, args, 0)?;
    let name = coerce_string(agent, name)?;
    Ok(agent
        .indexed_host_named_item(object, name)?
        .unwrap_or(Value::NULL))
}

#[derive(Clone, Copy)]
enum Filter {
    Children,
    Tag(StringId),
    Namespace(Option<StringId>, StringId),
    Classes(StringId, bool),
}

#[derive(Clone, Copy)]
struct Collection {
    root: Value,
    filter: Filter,
}

impl Collection {
    // A direct view holds the root wrapper. A descendant view instead holds a
    // private array with the root and the exact JS strings for its filter.
    fn from_source(agent: &mut Agent, source: Value) -> JsResult<Self> {
        let object = source.as_object().expect("collection source is an object");
        if agent.class(object) != Class::Array {
            return Ok(Self {
                root: source,
                filter: Filter::Children,
            });
        }
        let array = object;
        let root = agent.get(array, PropertyKey::Index(0))?;
        let kind = agent.get(array, PropertyKey::Index(1))?;
        let name = agent
            .get(array, PropertyKey::Index(2))?
            .as_string()
            .expect("collection query");
        let extra = agent.get(array, PropertyKey::Index(3))?;
        let filter = match kind.as_number() {
            Some(0.0) => Filter::Tag(name),
            Some(1.0) => Filter::Namespace(extra.as_string(), name),
            _ => Filter::Classes(name, extra == Value::TRUE),
        };
        Ok(Self { root, filter })
    }

    fn visit<T>(
        self,
        agent: &mut Agent,
        mut visit: impl FnMut(
            &mut Agent,
            &mut DomHeap,
            &Document,
            NodeId,
            &Element,
        ) -> JsResult<ControlFlow<T>>,
    ) -> JsResult<Option<T>> {
        let mut consider =
            |agent: &mut Agent, wrappers: &mut DomHeap, document: &Document, node_id| {
                agent.check_native_work()?;
                let node = document
                    .node(node_id)
                    .map_err(|error| dom_exception::from_dom_error(agent, error))?;
                let NodeKind::Element(element) = node.kind() else {
                    return Ok(ControlFlow::Continue(()));
                };
                if self.filter.matches(agent, element)? {
                    visit(agent, wrappers, document, node_id, element)
                } else {
                    Ok(ControlFlow::Continue(()))
                }
            };
        if matches!(self.filter, Filter::Children) {
            return collections::with_children(
                agent,
                self.root,
                collections::ChildKind::Elements,
                |agent, wrappers, document, ids| {
                    for &id in ids {
                        if let ControlFlow::Break(value) = consider(agent, wrappers, document, id)?
                        {
                            return Ok(Some(value));
                        }
                    }
                    Ok(None)
                },
            );
        }
        read_node(agent, self.root, |agent, wrappers, document, root, _| {
            let nodes = document
                .walk(root)
                .map_err(|error| dom_exception::from_dom_error(agent, error))?;
            for (id, _) in nodes.skip(1) {
                if let ControlFlow::Break(value) = consider(agent, wrappers, document, id)? {
                    return Ok(Some(value));
                }
            }
            Ok(None)
        })
    }

    fn find_name<T>(
        self,
        agent: &mut Agent,
        name: StringId,
        mut found: impl FnMut(&mut Agent, &mut DomHeap, &Document, NodeId) -> JsResult<T>,
    ) -> JsResult<Option<T>> {
        if agent.heap.string(name).is_empty() {
            return Ok(None);
        }
        self.visit(agent, |agent, wrappers, document, id, element| {
            for units in supported_names(agent, element)?.into_iter().flatten() {
                charge_units(agent, units.len())?;
                if units.iter().copied().eq(agent.heap.string(name).units()) {
                    return Ok(ControlFlow::Break(found(agent, wrappers, document, id)?));
                }
            }
            Ok(ControlFlow::Continue(()))
        })
    }

    fn name_keys(self, agent: &mut Agent) -> JsResult<Vec<StringId>> {
        let mut names = Vec::new();
        let mut seen = HashSet::new();
        self.visit(agent, |agent, _, _, _, element| {
            for units in supported_names(agent, element)?.into_iter().flatten() {
                charge_units(agent, units.len())?;
                let value = from_dom_units(agent, units)?;
                let key = agent
                    .heap
                    .key_for_string(value.as_string().expect("DOM string"))?;
                let PropertyKey::String(string) = key else {
                    continue;
                };
                if !seen.contains(&string) {
                    seen.try_reserve(1).map_err(|_| OutOfMemory)?;
                    names.try_reserve(1).map_err(|_| OutOfMemory)?;
                    seen.insert(string);
                    names.push(string);
                }
            }
            Ok(ControlFlow::<()>::Continue(()))
        })?;
        Ok(names)
    }
}

fn lookup_receiver<const DOCUMENT: bool>(agent: &mut Agent, root: Value) -> JsResult<()> {
    let brand: fn(&NodeKind) -> bool = if DOCUMENT {
        |kind| matches!(kind, NodeKind::Document)
    } else {
        |kind| matches!(kind, NodeKind::Element(_))
    };
    require_receiver(agent, root, brand, "invalid descendant collection receiver").map(|_| ())
}

fn descendants(
    agent: &mut Agent,
    root: Value,
    kind: u8,
    name: StringId,
    extra: Value,
) -> JsResult<Value> {
    let backing = agent.create_array_from_list(&[
        root,
        Value::number(f64::from(kind)),
        Value::string(name),
        extra,
    ])?;
    let proto = prototype(agent, HTML_COLLECTION)?;
    agent
        .new_indexed_view(HTML_COLLECTION, proto, Value::object(backing), &ACCESS)
        .map(Value::object)
}

fn by_tag<const DOCUMENT: bool>(agent: &mut Agent, root: Value, args: &[Value]) -> JsResult<Value> {
    lookup_receiver::<DOCUMENT>(agent, root)?;
    let name = required_arg(agent, args, 0)?;
    let name = coerce_string(agent, name)?;
    descendants(agent, root, 0, name, Value::UNDEFINED)
}

fn by_namespace<const DOCUMENT: bool>(
    agent: &mut Agent,
    root: Value,
    args: &[Value],
) -> JsResult<Value> {
    lookup_receiver::<DOCUMENT>(agent, root)?;
    let namespace = required_arg(agent, args, 0)?;
    let local = required_arg(agent, args, 1)?;
    let namespace = if namespace.is_nullish() {
        Value::NULL
    } else {
        let string = coerce_string(agent, namespace)?;
        if agent.heap.string(string).is_empty() {
            Value::NULL
        } else {
            Value::string(string)
        }
    };
    let namespace_root = agent.root(namespace)?;
    let result =
        coerce_string(agent, local).and_then(|name| descendants(agent, root, 1, name, namespace));
    agent.unroot(namespace_root);
    result
}

fn by_class<const DOCUMENT: bool>(
    agent: &mut Agent,
    root: Value,
    args: &[Value],
) -> JsResult<Value> {
    lookup_receiver::<DOCUMENT>(agent, root)?;
    let name = required_arg(agent, args, 0)?;
    let name = coerce_string(agent, name)?;
    let quirks = with_page(agent, |_, page| Ok(page.tree.quirks_mode()))?;
    descendants(
        agent,
        root,
        2,
        name,
        if quirks { Value::TRUE } else { Value::FALSE },
    )
}

fn live_length(agent: &mut Agent, source: Value) -> JsResult<u32> {
    let collection = Collection::from_source(agent, source)?;
    if matches!(collection.filter, Filter::Children) {
        return collections::child_len(agent, collection.root, collections::ChildKind::Elements);
    }
    let mut len = 0u32;
    collection.visit(agent, |_, _, _, _, _| {
        len = len.checked_add(1).ok_or(OutOfMemory)?;
        Ok(ControlFlow::<()>::Continue(()))
    })?;
    Ok(len)
}

fn live_item(agent: &mut Agent, source: Value, mut index: u32) -> JsResult<Option<Value>> {
    let collection = Collection::from_source(agent, source)?;
    if matches!(collection.filter, Filter::Children) {
        return collections::child_at(
            agent,
            collection.root,
            collections::ChildKind::Elements,
            index,
        );
    }
    collection.visit(agent, |agent, wrappers, document, id, _| {
        if index == 0 {
            return Ok(ControlFlow::Break(wrap(wrappers, agent, document, id)?));
        }
        index -= 1;
        Ok(ControlFlow::Continue(()))
    })
}

fn contains_name(agent: &mut Agent, source: Value, name: StringId) -> JsResult<bool> {
    Ok(Collection::from_source(agent, source)?
        .find_name(agent, name, |_, _, _, _| Ok(()))?
        .is_some())
}

fn live_named_item(agent: &mut Agent, source: Value, name: StringId) -> JsResult<Option<Value>> {
    Collection::from_source(agent, source)?.find_name(
        agent,
        name,
        |agent, wrappers, document, id| wrap(wrappers, agent, document, id),
    )
}

fn name_keys(agent: &mut Agent, source: Value) -> JsResult<Vec<StringId>> {
    Collection::from_source(agent, source)?.name_keys(agent)
}

fn supported_names<'a>(
    agent: &mut Agent,
    element: &'a Element,
) -> JsResult<[Option<&'a [u16]>; 2]> {
    let mut result = [None, None];
    for attribute in element.attributes() {
        agent.check_native_work()?;
        if attribute.name().namespace().is_none() && !attribute.value().as_units().is_empty() {
            match attribute.name().local() {
                "id" => result[0] = Some(attribute.value().as_units()),
                "name" if element.name().namespace() == Some(HTML_NAMESPACE) => {
                    result[1] = Some(attribute.value().as_units());
                }
                _ => {}
            }
        }
    }
    Ok(result)
}

impl Filter {
    fn matches(self, agent: &mut Agent, element: &Element) -> JsResult<bool> {
        let name = element.name();
        match self {
            Filter::Children => Ok(true),
            Filter::Tag(query) => {
                charge_units(agent, agent.heap.string(query).len())?;
                let query = agent.heap.string(query);
                let qualified = name
                    .prefix()
                    .into_iter()
                    .flat_map(str::encode_utf16)
                    .chain(name.prefix().map(|_| u16::from(b':')))
                    .chain(name.local().encode_utf16());
                Ok(query.units().eq([u16::from(b'*')])
                    || if name.namespace() == Some(HTML_NAMESPACE) {
                        query.units().map(ascii_lower).eq(qualified)
                    } else {
                        query.units().eq(qualified)
                    })
            }
            Filter::Namespace(namespace, local) => {
                charge_units(agent, agent.heap.string(local).len())?;
                let local = agent.heap.string(local);
                if !local.units().eq([u16::from(b'*')])
                    && !local.units().eq(name.local().encode_utf16())
                {
                    return Ok(false);
                }
                let Some(namespace) = namespace else {
                    return Ok(name.namespace().is_none());
                };
                charge_units(agent, agent.heap.string(namespace).len())?;
                let namespace = agent.heap.string(namespace);
                Ok(namespace.units().eq([u16::from(b'*')])
                    || name
                        .namespace()
                        .is_some_and(|ns| namespace.units().eq(ns.encode_utf16())))
            }
            Filter::Classes(query, quirks) => {
                for attribute in element.attributes() {
                    agent.check_native_work()?;
                    if attribute.name().namespace().is_none() && attribute.name().local() == "class"
                    {
                        return matches_classes(agent, query, attribute.value().as_units(), quirks);
                    }
                }
                Ok(false)
            }
        }
    }
}

fn charge_units(agent: &mut Agent, length: usize) -> JsResult<()> {
    for _ in (0..length).step_by(64) {
        agent.check_native_work()?;
    }
    Ok(())
}

fn ascii_lower(unit: u16) -> u16 {
    if matches!(unit, 0x41..=0x5a) {
        unit + 32
    } else {
        unit
    }
}

fn ascii_whitespace(unit: u16) -> bool {
    matches!(unit, 0x09 | 0x0a | 0x0c | 0x0d | 0x20)
}

fn matches_classes(
    agent: &mut Agent,
    query: StringId,
    attribute: &[u16],
    quirks: bool,
) -> JsResult<bool> {
    let length = agent.heap.string(query).len();
    charge_units(agent, length)?;
    let mut matched = false;
    let mut start = 0;
    for end in 0..=length {
        if agent
            .heap
            .string(query)
            .unit(end)
            .is_some_and(|unit| !ascii_whitespace(unit))
        {
            continue;
        }
        if start < end {
            charge_units(agent, attribute.len())?;
            let query = agent.heap.string(query);
            let token = || (start..end).map(|i| query.unit(i).expect("query token in bounds"));
            if !attribute
                .split(|&unit| ascii_whitespace(unit))
                .any(|class| {
                    class.len() == end - start
                        && if quirks {
                            class
                                .iter()
                                .copied()
                                .map(ascii_lower)
                                .eq(token().map(ascii_lower))
                        } else {
                            class.iter().copied().eq(token())
                        }
                })
            {
                return Ok(false);
            }
            matched = true;
        }
        start = end + 1;
    }
    Ok(matched)
}
