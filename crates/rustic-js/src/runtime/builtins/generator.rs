//! Generator objects (§27.5) and `%GeneratorFunction%` (§27.3).
//!
//! A generator object holds its suspended frame in a
//! [`Generator`](crate::runtime::object::Generator); the interpreter
//! resumes it. The methods here check the receiver and the state, then
//! hand over to [`Agent::generator_resume`].

use super::{Method, arg, install_methods, iterator_result, method, prototype_from_new_target};
use crate::runtime::FLAG_NATIVE_CONSTRUCTOR;
use crate::runtime::agent::{Abrupt, Agent, JsResult};
use crate::runtime::object::{GeneratorState, Internal, PropertyDescriptor};
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{ObjectId, Value};

/// How a generator is resumed: the `mode` a `yield` sees.
pub(crate) const RESUME_NEXT: u8 = 0;
pub(crate) const RESUME_THROW: u8 = 1;
pub(crate) const RESUME_RETURN: u8 = 2;

const PROTOTYPE_METHODS: &[Method] = &[
    method("next", generator_next, 1),
    method("return", generator_return, 1),
    method("throw", generator_throw, 1),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let iterator_prototype = agent
        .get_prototype_of(agent.intrinsics.array_iterator_prototype)?
        .expect("%ArrayIteratorPrototype% inherits from %IteratorPrototype%");
    let function_prototype = agent.intrinsics.function_prototype;
    let constructor_key = PropertyKey::String(agent.names.constructor);
    let prototype_key = PropertyKey::String(agent.names.prototype);
    let tag_key = PropertyKey::Symbol(agent.symbols.to_string_tag);

    // %GeneratorPrototype% (§27.5.1).
    let generator_prototype = agent.new_object_with(Some(iterator_prototype))?;
    install_methods(agent, generator_prototype, PROTOTYPE_METHODS)?;
    let tag = agent.string("Generator")?;
    agent.define_property_or_throw(
        generator_prototype,
        tag_key,
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;

    // %GeneratorFunction.prototype% (§27.3.3).
    let generator_function_prototype = agent.new_object_with(Some(function_prototype))?;
    agent.define_property_or_throw(
        generator_function_prototype,
        prototype_key,
        PropertyDescriptor::data_with(Value::object(generator_prototype), false, false, true),
    )?;
    agent.define_property_or_throw(
        generator_prototype,
        constructor_key,
        PropertyDescriptor::data_with(
            Value::object(generator_function_prototype),
            false,
            false,
            true,
        ),
    )?;
    let tag = agent.string("GeneratorFunction")?;
    agent.define_property_or_throw(
        generator_function_prototype,
        tag_key,
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;

    // %GeneratorFunction% (§27.3.1): not a global.
    let constructor = agent.new_native_function(generator_function, "GeneratorFunction", 1)?;
    agent
        .heap
        .object_mut(constructor)
        .set_flag(FLAG_NATIVE_CONSTRUCTOR, true);
    let function = agent.get(function_prototype, constructor_key)?;
    if let Some(function) = function.as_object() {
        agent.set_prototype_of(constructor, Some(function))?;
    }
    agent.define_property_or_throw(
        constructor,
        prototype_key,
        PropertyDescriptor::data_with(
            Value::object(generator_function_prototype),
            false,
            false,
            false,
        ),
    )?;
    agent.define_property_or_throw(
        generator_function_prototype,
        constructor_key,
        PropertyDescriptor::data_with(Value::object(constructor), false, false, true),
    )?;

    agent.intrinsics.generator_prototype = generator_prototype;
    agent.intrinsics.generator_function_prototype = generator_function_prototype;
    Ok(())
}

/// `GeneratorFunction(...params, body)` (§27.3.1.1).
fn generator_function(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let (params, body) = match args.split_last() {
        None => (&[][..], None),
        Some((body, params)) => (params, Some(*body)),
    };
    let mut parameter_text = Vec::with_capacity(params.len());
    for &p in params {
        let s = agent.to_string(p)?;
        parameter_text.push(agent.heap.string(s).to_text());
    }
    let body_text = match body {
        Some(b) => {
            let s = agent.to_string(b)?;
            agent.heap.string(s).to_text()
        }
        None => String::new(),
    };
    let f = agent.create_dynamic_function("function*", &parameter_text.join(","), &body_text)?;
    if let (Some(target), Some(o)) = (agent.new_target(), f.as_object()) {
        if agent.active_function() != Some(target) {
            let default = agent.intrinsics.generator_function_prototype;
            let prototype = prototype_from_new_target(agent, default)?;
            agent.set_prototype_of(o, Some(prototype))?;
        }
    }
    Ok(f)
}

/// `GeneratorValidate` (§27.5.3.2): the generator object and its state.
fn validate(agent: &mut Agent, this: Value) -> JsResult<(ObjectId, GeneratorState)> {
    if let Some(o) = this.as_object() {
        if let Some(Internal::Generator(g)) = agent.heap.object(o).internal() {
            if g.state == GeneratorState::Executing {
                return agent.type_error("the generator is already running");
            }
            return Ok((o, g.state));
        }
    }
    agent.type_error("not a generator object")
}

fn complete(agent: &mut Agent, o: ObjectId) {
    if let Some(Internal::Generator(g)) = agent.heap.object_mut(o).internal_mut() {
        g.state = GeneratorState::Completed;
    }
}

/// `%GeneratorPrototype%.next(value)` (§27.5.1.2).
fn generator_next(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, state) = validate(agent, this)?;
    if state == GeneratorState::Completed {
        return iterator_result(agent, Value::UNDEFINED, true);
    }
    agent.generator_resume(o, RESUME_NEXT, arg(args, 0))
}

/// `%GeneratorPrototype%.return(value)` (§27.5.1.3).
fn generator_return(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, state) = validate(agent, this)?;
    let value = arg(args, 0);
    match state {
        GeneratorState::SuspendedStart | GeneratorState::Completed => {
            complete(agent, o);
            iterator_result(agent, value, true)
        }
        _ => agent.generator_resume(o, RESUME_RETURN, value),
    }
}

/// `%GeneratorPrototype%.throw(exception)` (§27.5.1.4).
fn generator_throw(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (o, state) = validate(agent, this)?;
    let exception = arg(args, 0);
    match state {
        GeneratorState::SuspendedStart | GeneratorState::Completed => {
            complete(agent, o);
            Err(Abrupt::Throw(exception))
        }
        _ => agent.generator_resume(o, RESUME_THROW, exception),
    }
}
