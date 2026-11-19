//! HTML error reporting: `ErrorEvent`, `PromiseRejectionEvent`, the
//! `error`, `unhandledrejection` and `rejectionhandled` events at Window,
//! and the bounded `console` sink the host drains.
//!
//! [HTML §8.1.4.7](https://html.spec.whatwg.org/multipage/webappapis.html#runtime-script-errors)
//! [HTML §8.1.5.7](https://html.spec.whatwg.org/multipage/webappapis.html#unhandled-promise-rejections)

use std::cell::{Cell, RefCell};

use rustic_js::runtime::{
    Agent, JsResult, NativeFn, ObjectId, OutOfMemory, PropertyDescriptor, PropertyKey,
    RejectionEvent, RejectionOperation, Value,
};

use crate::webidl::{
    DomStringConversion, IntegerConversion, coerce_string, to_dom_string, to_unsigned_long,
};

use super::super::{install_interface, required_arg, with_page};
use super::state::{self, Event};
use super::{PrivateArray, Target, define_method, fire_with_target, option, with_root};

const ERROR_EVENT: &str = "ErrorEvent";
const REJECTION_EVENT: &str = "PromiseRejectionEvent";

/// Error event slots: message, filename, lineno, colno, error.
const ERROR_FIELDS: usize = 5;

/// Messages the console keeps before counting further ones as dropped.
pub const CONSOLE_CAPACITY: usize = 1024;
/// UTF-8 bytes one console message keeps; longer text is cut at a character
/// boundary and the message is marked truncated.
pub const CONSOLE_MESSAGE_BYTES: usize = 8 * 1024;

/// The severity a console method or the error reporter used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleLevel {
    /// `console.debug`.
    Debug,
    /// `console.log`.
    Log,
    /// `console.info`.
    Info,
    /// `console.warn`.
    Warn,
    /// `console.error`, failed `console.assert` and unreported page errors.
    Error,
}

/// One message for the host's developer console.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsoleMessage {
    /// Severity.
    pub level: ConsoleLevel,
    /// The formatted text.
    pub text: String,
    /// Whether `text` was cut to [`CONSOLE_MESSAGE_BYTES`].
    pub truncated: bool,
}

/// Messages taken from the page, and how many did not fit.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConsoleLog {
    /// Messages in order.
    pub messages: Vec<ConsoleMessage>,
    /// Messages discarded because [`CONSOLE_CAPACITY`] were waiting.
    pub dropped: u64,
}

/// Page-lifetime reporting state.
#[derive(Debug)]
pub(in crate::page) struct Reporting {
    errors: ObjectId,
    rejections: ObjectId,
    outstanding: ObjectId,
    reporting: Cell<bool>,
    console: RefCell<ConsoleLog>,
}

impl Reporting {
    pub(super) fn new(agent: &mut Agent) -> JsResult<(Self, [Value; 3])> {
        let errors = agent.new_weak_map()?;
        let rejections = agent.new_weak_map()?;
        let outstanding = agent.new_weak_map()?;
        Ok((
            Self {
                errors,
                rejections,
                outstanding,
                reporting: Cell::new(false),
                console: RefCell::new(ConsoleLog::default()),
            },
            [
                Value::object(errors),
                Value::object(rejections),
                Value::object(outstanding),
            ],
        ))
    }

    fn log(&self, level: ConsoleLevel, text: String) {
        let Ok(mut console) = self.console.try_borrow_mut() else {
            return;
        };
        if console.messages.len() >= CONSOLE_CAPACITY || console.messages.try_reserve(1).is_err() {
            console.dropped = console.dropped.saturating_add(1);
            return;
        }
        let truncated = text.len() > CONSOLE_MESSAGE_BYTES;
        let text = if truncated {
            let mut end = CONSOLE_MESSAGE_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text[..end].to_owned()
        } else {
            text
        };
        console.messages.push(ConsoleMessage {
            level,
            text,
            truncated,
        });
    }

    pub(in crate::page) fn take_console(&self) -> ConsoleLog {
        self.console
            .try_borrow_mut()
            .map(|mut log| std::mem::take(&mut *log))
            .unwrap_or_default()
    }
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        ERROR_EVENT,
        Some("Event"),
        construct_error,
        1,
        &[
            ("message", "get message", error_field::<0>),
            ("filename", "get filename", error_field::<1>),
            ("lineno", "get lineno", error_field::<2>),
            ("colno", "get colno", error_field::<3>),
            ("error", "get error", error_field::<4>),
        ],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        REJECTION_EVENT,
        Some("Event"),
        construct_rejection,
        2,
        &[
            ("promise", "get promise", rejection_field::<0>),
            ("reason", "get reason", rejection_field::<1>),
        ],
        |_, _| Ok(()),
    )?;
    install_console(agent)
}

fn construct_error(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("ErrorEvent constructor requires new");
    };
    let event_type = required_arg(agent, args, 0)?;
    let event_type = Value::string(coerce_string(agent, event_type)?);
    with_root(agent, event_type, |agent| {
        let options = args.get(1).copied().unwrap_or(Value::UNDEFINED);
        let (flags, _) = state::init_options::<false>(agent, options)?;
        // ErrorEventInit members in Web IDL order: colno, error, filename,
        // lineno, message.
        let mut fields = [Value::UNDEFINED; ERROR_FIELDS];
        fields[3] = Value::number(0.0);
        fields[2] = Value::number(0.0);
        let empty = agent.string("")?;
        fields[0] = empty;
        fields[1] = empty;
        let holder = PrivateArray::new(agent, &fields)?;
        with_root(agent, holder.value(), |agent| {
            if let Some(object) = options.as_object() {
                for (name, slot) in [
                    ("colno", 3),
                    ("error", 4),
                    ("filename", 1),
                    ("lineno", 2),
                    ("message", 0),
                ] {
                    let value = option(agent, object, name)?;
                    if value == Value::UNDEFINED {
                        continue;
                    }
                    let converted = match slot {
                        2 | 3 => Value::number(f64::from(to_unsigned_long(
                            agent,
                            value,
                            IntegerConversion::Wrap,
                        )?)),
                        4 => value,
                        _ => {
                            let text = to_dom_string(agent, value, DomStringConversion::Default)?;
                            crate::webidl::from_dom_string(agent, &text)?
                        }
                    };
                    holder.set(agent, slot, converted)?;
                }
            }
            let prototype = agent.host_prototype_from_new_target(ERROR_EVENT, new_target)?;
            let event = state::create(
                agent,
                prototype,
                event_type,
                flags,
                Value::NULL,
                Value::NULL,
            )?;
            with_page(agent, |agent, page| {
                agent.weak_map_set(page.events.reporting.errors, event, holder.value())
            })?;
            Ok(event)
        })
    })
}

fn construct_rejection(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("PromiseRejectionEvent constructor requires new");
    };
    let event_type = required_arg(agent, args, 0)?;
    let init = required_arg(agent, args, 1)?;
    let event_type = Value::string(coerce_string(agent, event_type)?);
    with_root(agent, event_type, |agent| {
        let (flags, _) = state::init_options::<false>(agent, init)?;
        let Some(object) = init.as_object() else {
            return agent.type_error("PromiseRejectionEventInit requires promise");
        };
        let promise = option(agent, object, "promise")?;
        if promise.as_object().is_none() {
            return agent.type_error("PromiseRejectionEventInit.promise must be an object");
        }
        with_root(agent, promise, |agent| {
            let reason = option(agent, object, "reason")?;
            let holder = PrivateArray::new(agent, &[promise, reason])?;
            with_root(agent, holder.value(), |agent| {
                let prototype =
                    agent.host_prototype_from_new_target(REJECTION_EVENT, new_target)?;
                let event = state::create(
                    agent,
                    prototype,
                    event_type,
                    flags,
                    Value::NULL,
                    Value::NULL,
                )?;
                with_page(agent, |agent, page| {
                    agent.weak_map_set(page.events.reporting.rejections, event, holder.value())
                })?;
                Ok(event)
            })
        })
    })
}

fn error_field<const SLOT: u32>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        let fields = agent.weak_map_get(page.events.reporting.errors, this);
        match PrivateArray::from_value(fields) {
            Some(fields) => fields.get(agent, SLOT),
            None => agent.type_error("expected an ErrorEvent receiver"),
        }
    })
}

fn rejection_field<const SLOT: u32>(
    agent: &mut Agent,
    this: Value,
    _: &[Value],
) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        let fields = agent.weak_map_get(page.events.reporting.rejections, this);
        match PrivateArray::from_value(fields) {
            Some(fields) => fields.get(agent, SLOT),
            None => agent.type_error("expected a PromiseRejectionEvent receiver"),
        }
    })
}

/// The ErrorEvent fields of `event`, if it is one, in handler argument order.
pub(super) fn error_event_fields(agent: &mut Agent, event: Value) -> JsResult<Option<[Value; 5]>> {
    with_page(agent, |agent, page| {
        let fields = agent.weak_map_get(page.events.reporting.errors, event);
        let Some(fields) = PrivateArray::from_value(fields) else {
            return Ok(None);
        };
        let mut out = [Value::UNDEFINED; ERROR_FIELDS];
        for (slot, value) in out.iter_mut().enumerate() {
            *value = fields.get(agent, slot as u32)?;
        }
        Ok(Some(out))
    })
}

/// HTML "report an exception": fire a cancelable `error` ErrorEvent at
/// Window and log the message unless a handler canceled it. An exception
/// thrown while reporting is only logged, so reporting cannot recurse.
pub(in crate::page) fn report_exception(agent: &mut Agent, error: Value) -> JsResult<()> {
    let text = agent.describe_value(error);
    let message = format!("Uncaught {text}");
    let (window, reentered) = with_page(agent, |_, page| {
        let reentered = page.events.reporting.reporting.replace(true);
        Ok((page.window, reentered))
    })?;
    if reentered {
        log(agent, ConsoleLevel::Error, message)?;
        return Ok(());
    }
    let result = with_root(agent, error, |agent| {
        let message_value = agent.string(&message)?;
        let empty = agent.string("")?;
        let holder = PrivateArray::new(
            agent,
            &[
                message_value,
                empty,
                Value::number(0.0),
                Value::number(0.0),
                error,
            ],
        )?;
        with_root(agent, holder.value(), |agent| {
            let event = state::trusted_event(
                agent,
                ERROR_EVENT,
                "error",
                Event::CANCELABLE,
                Value::NULL,
                Value::NULL,
            )?;
            with_root(agent, event, |agent| {
                with_page(agent, |agent, page| {
                    agent.weak_map_set(page.events.reporting.errors, event, holder.value())
                })?;
                let target = Target {
                    value: window,
                    node: false,
                };
                let state = Event::receiver(agent, event)?;
                fire_with_target(agent, target, event, state, window)
            })
        })
    });
    with_page(agent, |_, page| {
        page.events.reporting.reporting.set(false);
        Ok(())
    })?;
    if result? {
        log(agent, ConsoleLevel::Error, message)?;
    }
    Ok(())
}

/// HTML "notify about rejected promises" for the tracker's events since the
/// last checkpoint: `unhandledrejection` for each still-unhandled rejection,
/// and `rejectionhandled` for an earlier report that later gained a handler.
pub(in crate::page) fn report_rejections(
    agent: &mut Agent,
    events: Vec<RejectionEvent>,
) -> JsResult<()> {
    // Root every promise first: dispatch runs script, which can collect.
    let mut roots = Vec::new();
    roots
        .try_reserve_exact(events.len())
        .map_err(|_| OutOfMemory)?;
    for event in &events {
        roots.push(agent.root(Value::object(event.promise))?);
    }
    let mut result = Ok(());
    for event in &events {
        if result.is_err() {
            break;
        }
        result = report_rejection(agent, event);
    }
    for root in roots {
        agent.unroot(root);
    }
    result
}

fn report_rejection(agent: &mut Agent, event: &RejectionEvent) -> JsResult<()> {
    let promise = Value::object(event.promise);
    let outstanding = with_page(agent, |_, page| Ok(page.events.reporting.outstanding))?;
    match event.operation {
        RejectionOperation::Reject => {
            // A handler attached since the rejection was recorded wins.
            if agent.promise_is_handled(event.promise) == Some(true) {
                return Ok(());
            }
            let reason = agent
                .promise_result(event.promise)
                .unwrap_or(Value::UNDEFINED);
            let not_canceled = fire_rejection(agent, "unhandledrejection", promise, reason, true)?;
            if not_canceled {
                let text = agent.describe_value(reason);
                log(
                    agent,
                    ConsoleLevel::Error,
                    format!("Uncaught (in promise) {text}"),
                )?;
            }
            // A handler attached while the event ran handles it silently.
            if agent.promise_is_handled(event.promise) == Some(false) {
                agent.weak_map_set(outstanding, promise, Value::TRUE)?;
            }
        }
        RejectionOperation::Handle => {
            if agent.weak_map_delete(outstanding, promise) {
                let reason = agent
                    .promise_result(event.promise)
                    .unwrap_or(Value::UNDEFINED);
                fire_rejection(agent, "rejectionhandled", promise, reason, false)?;
            }
        }
    }
    Ok(())
}

fn fire_rejection(
    agent: &mut Agent,
    kind: &str,
    promise: Value,
    reason: Value,
    cancelable: bool,
) -> JsResult<bool> {
    let holder = PrivateArray::new(agent, &[promise, reason])?;
    with_root(agent, holder.value(), |agent| {
        let flags = if cancelable { Event::CANCELABLE } else { 0 };
        let event = state::trusted_event(
            agent,
            REJECTION_EVENT,
            kind,
            flags,
            Value::NULL,
            Value::NULL,
        )?;
        with_root(agent, event, |agent| {
            let window = with_page(agent, |agent, page| {
                agent.weak_map_set(page.events.reporting.rejections, event, holder.value())?;
                Ok(page.window)
            })?;
            let target = Target {
                value: window,
                node: false,
            };
            let state = Event::receiver(agent, event)?;
            fire_with_target(agent, target, event, state, window)
        })
    })
}

fn log(agent: &mut Agent, level: ConsoleLevel, text: String) -> JsResult<()> {
    with_page(agent, |_, page| {
        page.events.reporting.log(level, text);
        Ok(())
    })
}

fn install_console(agent: &mut Agent) -> JsResult<()> {
    let console = agent.new_object()?;
    let tag = Value::string(agent.heap.intern_str("console")?);
    agent.define_property_or_throw(
        console,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    for (name, method, length) in [
        ("debug", console_method::<0> as NativeFn, 0),
        ("log", console_method::<1>, 0),
        ("info", console_method::<2>, 0),
        ("warn", console_method::<3>, 0),
        ("error", console_method::<4>, 0),
        ("assert", console_assert, 0),
    ] {
        define_method(agent, console, name, method, length)?;
    }
    let key = agent.heap.key("console")?;
    agent.define_property_or_throw(
        agent.intrinsics.global,
        key,
        PropertyDescriptor::data_with(Value::object(console), true, false, true),
    )
}

fn level(index: u8) -> ConsoleLevel {
    match index {
        0 => ConsoleLevel::Debug,
        1 => ConsoleLevel::Log,
        2 => ConsoleLevel::Info,
        3 => ConsoleLevel::Warn,
        _ => ConsoleLevel::Error,
    }
}

/// Join arguments with spaces. Strings appear as they are; other values use
/// [`Agent::describe_value`], so logging never runs page code. Format
/// specifiers such as `%s` are not interpreted (TD-152).
fn format_args(agent: &mut Agent, args: &[Value]) -> String {
    let mut text = String::new();
    for (index, &value) in args.iter().enumerate() {
        if text.len() > CONSOLE_MESSAGE_BYTES {
            // The sink cuts the message; stop formatting work early.
            text.push(' ');
            break;
        }
        if index > 0 {
            text.push(' ');
        }
        text.push_str(&agent.describe_value(value));
    }
    text
}

fn console_method<const LEVEL: u8>(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    agent.check_native_work()?;
    let text = format_args(agent, args);
    log(agent, level(LEVEL), text)?;
    Ok(Value::UNDEFINED)
}

fn console_assert(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let condition = args.first().copied().unwrap_or(Value::UNDEFINED);
    if agent.to_boolean(condition) {
        return Ok(Value::UNDEFINED);
    }
    agent.check_native_work()?;
    let rest = format_args(agent, args.get(1..).unwrap_or(&[]));
    let text = if rest.is_empty() {
        "Assertion failed".to_owned()
    } else {
        format!("Assertion failed: {rest}")
    };
    log(agent, ConsoleLevel::Error, text)?;
    Ok(Value::UNDEFINED)
}
