use rustic_bindings::dom_exception;
use rustic_dom::{Document, DocumentLimits, DomError};
use rustic_js::runtime::{Abrupt, Agent, JsResult, RealmId, Roots, Value};

#[test]
fn installed_interface_has_web_idl_properties_and_legacy_codes() {
    let mut agent = Agent::new().unwrap();
    assert_eq!(
        agent
            .evaluate("typeof DOMException === 'undefined'")
            .unwrap(),
        Value::TRUE
    );
    dom_exception::install(&mut agent).unwrap();
    assert_eq!(
        agent.evaluate(r#"
            const ctor = DOMException;
            const p = ctor.prototype;
            const global = Object.getOwnPropertyDescriptor(globalThis, "DOMException");
            if (!global.writable || global.enumerable || !global.configurable) throw "global";
            if (ctor.name !== "DOMException" || ctor.length !== 0 || p.constructor !== ctor)
                throw "constructor";
            if (Object.getPrototypeOf(ctor) !== Function.prototype ||
                Object.getPrototypeOf(p) !== Error.prototype || !Error.isError(p))
                throw "prototype";
            for (const key of ["name", "message", "code"]) {
                const attr = Object.getOwnPropertyDescriptor(p, key);
                if (!attr.enumerable || !attr.configurable || attr.set !== undefined ||
                    attr.get.name !== "get " + key || attr.get.length !== 0)
                    throw key;
            }
            const tag = Object.getOwnPropertyDescriptor(p, Symbol.toStringTag);
            if (tag.value !== "DOMException" || tag.writable || tag.enumerable || !tag.configurable)
                throw "tag";
            const constants = [
                "INDEX_SIZE_ERR", "DOMSTRING_SIZE_ERR", "HIERARCHY_REQUEST_ERR",
                "WRONG_DOCUMENT_ERR", "INVALID_CHARACTER_ERR", "NO_DATA_ALLOWED_ERR",
                "NO_MODIFICATION_ALLOWED_ERR", "NOT_FOUND_ERR", "NOT_SUPPORTED_ERR",
                "INUSE_ATTRIBUTE_ERR", "INVALID_STATE_ERR", "SYNTAX_ERR",
                "INVALID_MODIFICATION_ERR", "NAMESPACE_ERR", "INVALID_ACCESS_ERR",
                "VALIDATION_ERR", "TYPE_MISMATCH_ERR", "SECURITY_ERR", "NETWORK_ERR",
                "ABORT_ERR", "URL_MISMATCH_ERR", "QUOTA_EXCEEDED_ERR", "TIMEOUT_ERR",
                "INVALID_NODE_TYPE_ERR", "DATA_CLONE_ERR"
            ];
            for (let i = 0; i < constants.length; i++) {
                for (const object of [ctor, p]) {
                    const desc = Object.getOwnPropertyDescriptor(object, constants[i]);
                    if (desc.value !== i + 1 || desc.writable || !desc.enumerable || desc.configurable)
                        throw constants[i];
                }
            }
            const defaults = new DOMException(undefined, undefined);
            defaults.message === "" && defaults.name === "Error" && defaults.code === 0 &&
            new DOMException("", "TimeoutError").code === 23 &&
            new DOMException("", "QuotaExceededError").code === 22 &&
            new DOMException("", "DOMStringSizeError").code === 0 &&
            new DOMException("", "ValidationError").code === 0 &&
            new DOMException("", "NoDataAllowedError").code === 0 &&
            new DOMException(null, null).message === "null" &&
            new DOMException("", "timeoutError").code === 0;
        "#).unwrap(),
        Value::TRUE
    );
}

#[test]
fn constructor_preserves_utf16_order_user_throws_and_brand_checks() {
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    agent.heap.set_gc_stress(true);
    assert_eq!(
        agent
            .evaluate(
                r#"
            let order = "";
            class Child extends DOMException {}
            const child = new Child({
                toString() { order += "m"; return "\uD800"; }
            }, {
                toString() { order += "n"; return "AbortError"; }
            });
            if (!(child instanceof Child) || !(child instanceof DOMException) ||
                !(child instanceof Error) || !Error.isError(child) ||
                Object.prototype.toString.call(child) !== "[object DOMException]")
                throw "brand";
            const getter = Object.getOwnPropertyDescriptor(DOMException.prototype, "code").get;
            let typeErrors = 0;
            for (const operation of [
                () => DOMException({ toString() { throw "called without new"; } }),
                () => getter.call(DOMException.prototype),
                () => getter.call(Object.create(DOMException.prototype)),
                () => getter.call(new Proxy(child, {})),
                () => new DOMException(Symbol())
            ]) {
                try { operation(); } catch (error) {
                    if (!(error instanceof TypeError)) throw error;
                    typeErrors++;
                }
            }
            const sentinel = {};
            try {
                new DOMException("", { toString() { throw sentinel; } });
                throw "coercion did not throw";
            } catch (error) {
                if (error !== sentinel) throw error;
            }
            order === "mn" && typeErrors === 5 && getter.call(child) === 20 &&
            child.message.charCodeAt(0) === 0xd800 &&
            child.toString() === "AbortError: \uD800";
        "#
            )
            .unwrap(),
        Value::TRUE
    );
}

#[test]
fn interface_prototypes_and_exception_strings_survive_collection_in_each_realm() {
    let mut agent = Agent::new().unwrap();
    let realm = agent.create_realm().unwrap();
    dom_exception::install(&mut agent).unwrap();
    let main = agent.host_interface_prototype("DOMException").unwrap();
    let ctor = agent.evaluate("DOMException").unwrap();
    let ctor_root = agent.root(ctor).unwrap();
    agent.enter_realm(realm);
    dom_exception::install(&mut agent).unwrap();
    let foreign = agent.host_interface_prototype("DOMException").unwrap();
    assert_ne!(main, foreign);

    // An invalid newTarget.prototype falls back to the interface in its realm.
    let target = agent
        .evaluate("function Target() {} Target.prototype = null; Target;")
        .unwrap();
    let e = agent
        .construct_with(ctor, &[], target.as_object().unwrap())
        .unwrap();
    assert_eq!(
        agent.stored_prototype(e.as_object().unwrap()),
        Some(foreign)
    );
    assert!(agent.unroot(ctor_root));

    agent
        .evaluate("delete DOMException.prototype.constructor; delete globalThis.DOMException")
        .unwrap();
    agent.collect_garbage(Roots::default());
    dom_exception::install(&mut agent).unwrap();
    assert_eq!(
        agent
            .evaluate("typeof DOMException === 'undefined'")
            .unwrap(),
        Value::TRUE
    );
    let exception = dom_exception::new(&mut agent, "unique-name", "unique-message").unwrap();
    let reference = agent.root(exception).unwrap();
    agent.enter_realm(RealmId::MAIN);
    agent.collect_garbage(Roots::default());
    let exception = agent.rooted(reference).unwrap().as_object().unwrap();
    assert_eq!(agent.stored_prototype(exception), Some(foreign));
    for (name, expected) in [("name", "unique-name"), ("message", "unique-message")] {
        let key = agent.heap.key(name).unwrap();
        let value = agent.get(exception, key).unwrap();
        assert_eq!(
            agent.heap.string(value.as_string().unwrap()).to_text(),
            expected
        );
    }
    assert!(agent.unroot(reference));
    assert!(agent.collect_garbage(Roots::default()).objects_freed > 0);
}

#[test]
fn host_error_conversion_uses_a_real_dom_operation_and_preserves_fatal_failures() {
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    let key = agent.heap.key("replacePastEnd").unwrap();
    agent
        .define_method(
            agent.intrinsics.global,
            key,
            replace_past_end,
            "replacePastEnd",
            0,
        )
        .unwrap();
    assert_eq!(
        agent
            .evaluate(
                r#"
            let caught = false;
            try { replacePastEnd(); } catch (error) {
                caught = error instanceof DOMException && error.name === "IndexSizeError" &&
                    error.code === DOMException.INDEX_SIZE_ERR && error.message.length > 0;
            }
            caught;
        "#
            )
            .unwrap(),
        Value::TRUE
    );
    for error in [DomError::IdentityExhausted, DomError::AllocationFailed] {
        assert_eq!(
            dom_exception::from_dom_error(&mut agent, error),
            Abrupt::OutOfMemory
        );
    }
}

fn replace_past_end(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let convert = |agent: &mut Agent, error| dom_exception::from_dom_error(agent, error);
    let mut doc = Document::new(DocumentLimits::default()).map_err(|e| convert(agent, e))?;
    let text = doc.create_text("a").map_err(|e| convert(agent, e))?;
    doc.replace_character_data(text, 2, 1, &[0x62])
        .map_err(|e| convert(agent, e))?;
    Ok(Value::UNDEFINED)
}

#[test]
fn failed_installation_does_not_register_a_partial_interface() {
    let mut agent = Agent::new().unwrap();
    agent
        .evaluate("Object.defineProperty(globalThis, 'DOMException', { value: 42 });")
        .unwrap();
    assert!(matches!(
        dom_exception::install(&mut agent),
        Err(Abrupt::Throw(_))
    ));
    assert_eq!(agent.host_interface_prototype("DOMException"), None);
    assert_eq!(agent.evaluate("DOMException").unwrap(), Value::number(42.0));
}
