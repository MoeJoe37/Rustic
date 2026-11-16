use rustic_bindings::webidl::{
    DomStringConversion, IntegerConversion, from_dom_string, to_byte_string, to_dom_string,
    to_long, to_nullable_dom_string, to_unsigned_long, to_usv_string,
};
use rustic_dom::DomString;
use rustic_js::runtime::{Abrupt, Agent, JsResult, PropertyDescriptor, PropertyKey, Roots, Value};

#[test]
fn string_kinds_apply_their_distinct_code_unit_rules() {
    let mut agent = Agent::new().unwrap();
    let original = DomString::from_units(vec![0x41, 0xff, 0xd800, 0xd83d, 0xde80, 0xdc00]);
    let string = from_dom_string(&mut agent, &original).unwrap();
    assert_eq!(
        to_dom_string(&mut agent, string, DomStringConversion::Default).unwrap(),
        original
    );
    assert_eq!(to_usv_string(&mut agent, string).unwrap(), "Aÿ�🚀�");

    let bytes = DomString::from_units(vec![0, 0x41, 0xff]);
    let bytes = from_dom_string(&mut agent, &bytes).unwrap();
    assert_eq!(to_byte_string(&mut agent, bytes).unwrap(), [0, 0x41, 0xff]);
    let invalid_byte = agent.string("Ā").unwrap();
    let rejected = to_byte_string(&mut agent, invalid_byte);
    assert_type_error(&agent, rejected);
}

#[test]
fn nullable_and_legacy_null_conversions_are_distinct() {
    let mut agent = Agent::new().unwrap();
    for nullish in [Value::NULL, Value::UNDEFINED] {
        assert_eq!(to_nullable_dom_string(&mut agent, nullish).unwrap(), None);
    }
    assert_eq!(
        to_dom_string(&mut agent, Value::NULL, DomStringConversion::Default).unwrap(),
        DomString::from("null")
    );
    assert_eq!(
        to_dom_string(
            &mut agent,
            Value::NULL,
            DomStringConversion::LegacyNullToEmptyString
        )
        .unwrap(),
        DomString::default()
    );
    assert_eq!(
        to_dom_string(
            &mut agent,
            Value::UNDEFINED,
            DomStringConversion::LegacyNullToEmptyString
        )
        .unwrap(),
        DomString::from("undefined")
    );
    let literal = agent.string("null").unwrap();
    assert_eq!(
        to_nullable_dom_string(&mut agent, literal).unwrap(),
        Some(DomString::from("null"))
    );
}

#[test]
fn integer_modes_wrap_clamp_and_enforce_range_at_the_boundaries() {
    use IntegerConversion::{Clamp, EnforceRange, Wrap};

    let mut agent = Agent::new().unwrap();
    for (input, expected) in [
        (2_147_483_648.0, i32::MIN),
        (-2_147_483_649.0, i32::MAX),
        (-1.9, -1),
        (-0.0, 0),
        (f64::MAX, 0),
        (f64::INFINITY, 0),
        (f64::NAN, 0),
    ] {
        assert_eq!(
            to_long(&mut agent, Value::number(input), Wrap).unwrap(),
            expected
        );
    }
    for (input, expected) in [(-1.9, u32::MAX), (4_294_967_296.0, 0), (42.9, 42)] {
        assert_eq!(
            to_unsigned_long(&mut agent, Value::number(input), Wrap).unwrap(),
            expected
        );
    }
    for (input, expected) in [
        (1.5, 2),
        (2.5, 2),
        (-2.5, -2),
        (-3.5, -4),
        (f64::NEG_INFINITY, i32::MIN),
        (f64::INFINITY, i32::MAX),
        (f64::NAN, 0),
    ] {
        assert_eq!(
            to_long(&mut agent, Value::number(input), Clamp).unwrap(),
            expected
        );
    }
    assert_eq!(
        to_unsigned_long(&mut agent, Value::number(-10.0), Clamp).unwrap(),
        0
    );
    assert_eq!(
        to_unsigned_long(&mut agent, Value::number(f64::INFINITY), Clamp).unwrap(),
        u32::MAX
    );
    assert_eq!(
        to_unsigned_long(&mut agent, Value::number(-0.75), EnforceRange).unwrap(),
        0
    );
    assert_eq!(
        to_unsigned_long(&mut agent, Value::number(4_294_967_295.75), EnforceRange).unwrap(),
        u32::MAX
    );
    for input in [-1.0, 4_294_967_296.0, f64::INFINITY, f64::NAN] {
        let rejected = to_unsigned_long(&mut agent, Value::number(input), EnforceRange);
        assert_type_error(&agent, rejected);
    }
    for input in [-2_147_483_649.0, 2_147_483_648.0, f64::NEG_INFINITY] {
        let rejected = to_long(&mut agent, Value::number(input), EnforceRange);
        assert_type_error(&agent, rejected);
    }
}

#[test]
fn host_coercion_roots_input_across_collecting_getters() {
    let mut agent = Agent::new().unwrap();
    let object = agent
        .evaluate("({valueOf() { return 'valueOf after getter'; }})")
        .unwrap();
    let getter = agent
        .new_native_function(collecting_getter, "get toString", 0)
        .unwrap();
    agent
        .define_property_or_throw(
            object.as_object().unwrap(),
            PropertyKey::String(agent.names.to_string),
            PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, true, true),
        )
        .unwrap();
    let before = agent.heap.collections();
    assert_eq!(
        to_dom_string(&mut agent, object, DomStringConversion::Default).unwrap(),
        DomString::from("valueOf after getter")
    );
    assert!(agent.heap.collections() > before);
}

fn collecting_getter(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    // This host callback enters from a test with no active script frames.
    // The fallback reads valueOf on the input object after this collection.
    agent.collect_garbage(Roots::default());
    Ok(Value::NULL)
}

#[test]
fn native_bindings_keep_user_exceptions_and_throw_type_errors_in_javascript() {
    let mut agent = Agent::new().unwrap();
    let key = agent.heap.key("idlString").unwrap();
    let global = agent.intrinsics.global;
    agent
        .define_method(global, key, native_dom_string, "idlString", 1)
        .unwrap();
    let key = agent.heap.key("idlIndex").unwrap();
    agent
        .define_method(global, key, native_index, "idlIndex", 1)
        .unwrap();
    agent.heap.set_gc_stress(true);
    let result = agent
        .evaluate(
            r#"
            let sentinel = {};
            let caught = 0;
            try {
              idlString({ [Symbol.toPrimitive](hint) {
                if (hint !== "string") throw "wrong hint";
                throw sentinel;
              } });
            } catch (error) {
              if (error !== sentinel) throw error;
              caught++;
            }
            try { idlString(Symbol()); } catch (error) {
              if (!(error instanceof TypeError)) throw error;
              caught++;
            }
            try { idlIndex(4294967296); } catch (error) {
              if (!(error instanceof TypeError)) throw error;
              caught++;
            }
            let number = idlIndex({ valueOf() { return "23.75"; } });
            let string = idlString({ toString() { return "\uD800"; } });
            caught === 3 && number === 23 && string.charCodeAt(0) === 0xd800;
            "#,
        )
        .unwrap();
    assert_eq!(result, Value::TRUE);
}

fn native_dom_string(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let converted = to_dom_string(agent, value, DomStringConversion::Default)?;
    from_dom_string(agent, &converted)
}

fn native_index(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    to_unsigned_long(agent, value, IntegerConversion::EnforceRange)
        .map(|number| Value::number(f64::from(number)))
}

fn assert_type_error<T: std::fmt::Debug>(agent: &Agent, result: JsResult<T>) {
    let Err(Abrupt::Throw(error)) = result else {
        panic!("expected a thrown TypeError, got {result:?}");
    };
    assert_eq!(
        agent.stored_prototype(error.as_object().unwrap()),
        Some(agent.intrinsics.type_error_prototype)
    );
}
