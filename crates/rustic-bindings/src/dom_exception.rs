//! The Web IDL DOMException interface and conversion of document errors.

use rustic_dom::DomError;
use rustic_js::runtime::{
    Abrupt, Agent, JsResult, NativeFn, ObjectId, PropertyDescriptor, PropertyKey, StringId, Value,
};

use crate::webidl::coerce_string;

const INTERFACE: &str = "DOMException";

// Web IDL §§2.8.1 and 4.4. The constants for 2, 6 and 16 have no
// corresponding name. Each index plus one is the legacy code.
const LEGACY_CODES: [(&str, Option<&str>); 25] = [
    ("INDEX_SIZE_ERR", Some("IndexSizeError")),
    ("DOMSTRING_SIZE_ERR", None),
    ("HIERARCHY_REQUEST_ERR", Some("HierarchyRequestError")),
    ("WRONG_DOCUMENT_ERR", Some("WrongDocumentError")),
    ("INVALID_CHARACTER_ERR", Some("InvalidCharacterError")),
    ("NO_DATA_ALLOWED_ERR", None),
    (
        "NO_MODIFICATION_ALLOWED_ERR",
        Some("NoModificationAllowedError"),
    ),
    ("NOT_FOUND_ERR", Some("NotFoundError")),
    ("NOT_SUPPORTED_ERR", Some("NotSupportedError")),
    ("INUSE_ATTRIBUTE_ERR", Some("InUseAttributeError")),
    ("INVALID_STATE_ERR", Some("InvalidStateError")),
    ("SYNTAX_ERR", Some("SyntaxError")),
    ("INVALID_MODIFICATION_ERR", Some("InvalidModificationError")),
    ("NAMESPACE_ERR", Some("NamespaceError")),
    ("INVALID_ACCESS_ERR", Some("InvalidAccessError")),
    ("VALIDATION_ERR", None),
    ("TYPE_MISMATCH_ERR", Some("TypeMismatchError")),
    ("SECURITY_ERR", Some("SecurityError")),
    ("NETWORK_ERR", Some("NetworkError")),
    ("ABORT_ERR", Some("AbortError")),
    ("URL_MISMATCH_ERR", Some("URLMismatchError")),
    ("QUOTA_EXCEEDED_ERR", Some("QuotaExceededError")),
    ("TIMEOUT_ERR", Some("TimeoutError")),
    ("INVALID_NODE_TYPE_ERR", Some("InvalidNodeTypeError")),
    ("DATA_CLONE_ERR", Some("DataCloneError")),
];

/// Install the base `DOMException` interface in the agent's current
/// realm. Installing it again preserves the realm's original interface.
pub fn install(agent: &mut Agent) -> JsResult<()> {
    if agent.host_interface_prototype(INTERFACE).is_some() {
        return Ok(());
    }
    let prototype = agent.new_error_object_with(agent.intrinsics.error_prototype)?;
    let getters: [(&str, &str, NativeFn); 3] = [
        ("name", "get name", name),
        ("message", "get message", message),
        ("code", "get code", code),
    ];
    for (property, function_name, behavior) in getters {
        let getter = agent.new_native_function(behavior, function_name, 0)?;
        let key = agent.heap.key(property)?;
        agent.define_property_or_throw(
            prototype,
            key,
            PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, true, true),
        )?;
    }
    let tag = PropertyKey::Symbol(agent.symbols.to_string_tag);
    let tag_value = Value::string(agent.heap.intern_str(INTERFACE)?);
    agent.define_property_or_throw(
        prototype,
        tag,
        PropertyDescriptor::data_with(tag_value, false, false, true),
    )?;
    install_constants(agent, prototype)?;
    agent.install_host_interface(INTERFACE, construct, 0, prototype, install_constants)
}

fn install_constants(agent: &mut Agent, target: ObjectId) -> JsResult<()> {
    for (index, (constant, _)) in LEGACY_CODES.iter().enumerate() {
        let key = agent.heap.key(constant)?;
        let value = Value::number(f64::from(legacy_code(index)));
        agent.define_property_or_throw(
            target,
            key,
            PropertyDescriptor::data_with(value, false, true, false),
        )?;
    }
    Ok(())
}

fn legacy_code(index: usize) -> u16 {
    u16::try_from(index + 1).expect("DOMException legacy table index")
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("DOMException requires new");
    };
    let message = string_argument(agent, args.first(), "")?;
    // Later conversions and the prototype lookup can call script.
    let message_root = agent.root(Value::string(message))?;
    let result = (|| {
        let name = string_argument(agent, args.get(1), "Error")?;
        let name_root = agent.root(Value::string(name))?;
        let prototype = agent.host_prototype_from_new_target(INTERFACE, target);
        agent.unroot(name_root);
        agent.new_host_error(INTERFACE, prototype?, name, message)
    })();
    agent.unroot(message_root);
    result
}

fn string_argument(agent: &mut Agent, input: Option<&Value>, default: &str) -> JsResult<StringId> {
    match input.copied() {
        None | Some(Value::UNDEFINED) => Ok(agent.heap.intern_str(default)?),
        Some(value) => coerce_string(agent, value),
    }
}

fn data(agent: &mut Agent, receiver: Value) -> JsResult<(StringId, StringId)> {
    match receiver
        .as_object()
        .and_then(|object| agent.host_error_data(object, INTERFACE))
    {
        Some(data) => Ok(data),
        None => agent.type_error("DOMException getter requires a DOMException receiver"),
    }
}

fn name(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::string(data(agent, receiver)?.0))
}

fn message(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::string(data(agent, receiver)?.1))
}

fn code(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    let (name, _) = data(agent, receiver)?;
    let units = agent.heap.string(name);
    let code = LEGACY_CODES
        .iter()
        .position(|(_, name)| {
            name.is_some_and(|candidate| units.units().eq(candidate.bytes().map(u16::from)))
        })
        .map_or(0, legacy_code);
    Ok(Value::number(f64::from(code)))
}

/// Create a base `DOMException` using the installed interface prototype in
/// the current realm. The global constructor can be replaced by script.
pub fn new(agent: &mut Agent, name: &str, message: &str) -> JsResult<Value> {
    let Some(prototype) = agent.host_interface_prototype(INTERFACE) else {
        return agent.type_error("DOMException is not installed in this realm");
    };
    let name = agent.heap.alloc_str(name)?;
    let message = agent.heap.alloc_str(message)?;
    agent.new_host_error(INTERFACE, prototype, name, message)
}

/// Convert a DOM failure to an abrupt completion in the current realm.
/// Arena allocation and identity exhaustion remain fatal. Configured
/// tree and payload quotas use the base `QuotaExceededError` name.
pub fn from_dom_error(agent: &mut Agent, error: DomError) -> Abrupt {
    let name = match error {
        DomError::WrongDocument => "WrongDocumentError",
        DomError::InvalidNode | DomError::InvalidState => "InvalidStateError",
        DomError::InvalidHierarchy | DomError::Cycle => "HierarchyRequestError",
        DomError::InvalidReference => "NotFoundError",
        DomError::ProtectedRoot => "NotSupportedError",
        DomError::DepthLimit | DomError::NodeLimit | DomError::DataLimit => "QuotaExceededError",
        DomError::WrongNodeKind => "InvalidNodeTypeError",
        DomError::InvalidOffset => "IndexSizeError",
        DomError::InvalidName | DomError::InvalidCharacterData => "InvalidCharacterError",
        DomError::IdentityExhausted | DomError::AllocationFailed => return Abrupt::OutOfMemory,
    };
    // DomError's Display text consists only of fixed engine descriptions.
    match new(agent, name, &error.to_string()) {
        Ok(exception) => Abrupt::Throw(exception),
        Err(abrupt) => abrupt,
    }
}
