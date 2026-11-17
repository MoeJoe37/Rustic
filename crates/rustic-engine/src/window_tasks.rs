//! Window timer code, callback roots and scheduler entry points.

use std::cell::RefCell;
use std::time::Duration;

use rustic_bindings::page::{TimerHandler, WindowScheduler};
use rustic_js::runtime::{
    Abrupt, Agent, ErrorKind, HostRoot, JsResult, OutOfMemory, PropertyKey, Value,
};
use rustic_js::{EvalError, Source};

use crate::ScriptQueueError;
use crate::tasks::ScriptQueue;

#[derive(Debug)]
pub(crate) struct WindowWork {
    pub(crate) queue: RefCell<ScriptQueue>,
}

impl WindowScheduler for WindowWork {
    fn set_timer(
        &self,
        agent: &mut Agent,
        handler: TimerHandler<'_>,
        delay: Duration,
        repeat: bool,
    ) -> JsResult<i32> {
        let units = match handler {
            TimerHandler::Source(source) => agent.heap.string(source).len(),
            TimerHandler::Function(..) => 0,
        };
        if units > rustic_js::MAX_SOURCE_UNITS {
            return Err(queue_error(agent, ScriptQueueError::Limit));
        }
        let mut queue = self.queue.borrow_mut();
        let id = queue
            .next_window_timer()
            .map_err(|error| queue_error(agent, error))?;
        let nesting = queue.active_nesting;
        queue
            .check_admission(units)
            .map_err(|error| queue_error(agent, error))?;
        let code = TimerCode::retain(agent, handler)?;
        let timer = WindowTimer {
            id,
            code,
            units,
            nesting: nesting.saturating_add(1),
            interval: repeat.then_some(delay),
        };
        if let Err(error) = queue.stage_timer(timer, clamp_delay(delay, nesting)) {
            code.release(agent);
            return Err(queue_error(agent, error));
        }
        queue.commit_window_timer(id);
        Ok(id)
    }

    fn clear_timer(&self, agent: &mut Agent, id: i32) -> JsResult<()> {
        if let Some(timer) = self.queue.borrow_mut().cancel_window_timer(agent, id)? {
            timer.code.release(agent);
        }
        Ok(())
    }

    fn request_frame(&self, agent: &mut Agent, callback: Value) -> JsResult<u32> {
        let mut queue = self.queue.borrow_mut();
        let id = queue
            .reserve_frame()
            .map_err(|error| queue_error(agent, error))?;
        let root = agent.root(callback)?;
        queue.queue_frame(id, root);
        Ok(id)
    }

    fn cancel_frame(&self, agent: &mut Agent, id: u32) -> JsResult<()> {
        if let Some(root) = self.queue.borrow_mut().cancel_frame(agent, id)? {
            agent.unroot(root);
        }
        Ok(())
    }
}

fn queue_error(agent: &mut Agent, error: ScriptQueueError) -> Abrupt {
    match error {
        ScriptQueueError::Capacity => OutOfMemory.into(),
        _ => agent.throw(ErrorKind::RangeError, &error.to_string()),
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum TimerCode {
    Function { root: HostRoot, arguments: u32 },
    Source(HostRoot),
}

impl TimerCode {
    fn retain(agent: &mut Agent, handler: TimerHandler<'_>) -> JsResult<Self> {
        match handler {
            TimerHandler::Function(callback, args) => {
                let length = args.len().checked_add(1).ok_or(OutOfMemory)?;
                let arguments = u32::try_from(length).map_err(|_| OutOfMemory)? - 1;
                let mut record = Vec::new();
                record.try_reserve_exact(length).map_err(|_| OutOfMemory)?;
                record.push(callback);
                for &arg in args {
                    agent.check_native_work()?;
                    record.push(arg);
                }
                let record = agent.create_array_from_list(&record)?;
                let root = agent.root(Value::object(record))?;
                Ok(Self::Function { root, arguments })
            }
            TimerHandler::Source(source) => agent
                .root(Value::string(source))
                .map(Self::Source)
                .map_err(Into::into),
        }
    }

    pub(crate) fn release(self, agent: &mut Agent) {
        let root = match self {
            Self::Function { root, .. } | Self::Source(root) => root,
        };
        agent.unroot(root);
    }

    fn run(self, agent: &mut Agent) -> Result<(), EvalError> {
        agent.check_native_work()?;
        match self {
            Self::Function { root, arguments } => {
                let record = agent
                    .rooted(root)
                    .and_then(Value::as_object)
                    .expect("retained timer record");
                let callback = agent.get(record, PropertyKey::Index(0))?;
                let mut args = Vec::new();
                args.try_reserve_exact(arguments as usize)
                    .map_err(|_| EvalError::OutOfMemory)?;
                for index in 1..=arguments {
                    agent.check_native_work()?;
                    args.push(agent.get(record, PropertyKey::Index(index))?);
                }
                let window = Value::object(agent.global_this());
                agent.call(callback, window, &args)?;
            }
            Self::Source(root) => {
                let source = agent
                    .rooted(root)
                    .and_then(Value::as_string)
                    .expect("retained timer source");
                let len = agent.heap.string(source).len();
                for _ in (0..len).step_by(256) {
                    agent.check_native_work()?;
                }
                let source = agent.heap.string(source);
                let mut units = Vec::new();
                units
                    .try_reserve_exact(source.len())
                    .map_err(|_| EvalError::OutOfMemory)?;
                units.extend(source.units());
                let source = Source::from_utf16(units, rustic_js::MAX_SOURCE_UNITS)
                    .map_err(|_| EvalError::SourceTooLong)?;
                agent.evaluate_source(&source)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WindowTimer {
    pub(crate) id: i32,
    pub(crate) code: TimerCode,
    pub(crate) units: usize,
    pub(crate) nesting: u8,
    interval: Option<Duration>,
}

impl WindowTimer {
    pub(crate) fn run(self, agent: &mut Agent, pending: &WindowWork) -> Result<(), EvalError> {
        {
            let mut queue = pending.queue.borrow_mut();
            if self.interval.is_some() {
                // Keep the interval registered and budgeted during its handler.
                queue.start_interval(self);
            }
            queue.active_nesting = self.nesting;
        }
        let result = self.code.run(agent);
        let mut queue = pending.queue.borrow_mut();
        queue.active_nesting = 0;
        let Some(delay) = self.interval else {
            self.code.release(agent);
            return result;
        };
        let Some(mut timer) = queue.take_interval() else {
            return result;
        };
        if matches!(result, Err(EvalError::Cancelled | EvalError::OutOfMemory)) {
            timer.code.release(agent);
            return result;
        }
        timer.nesting = self.nesting.saturating_add(1);
        if let Err(error) = queue.stage_timer(timer, clamp_delay(delay, self.nesting)) {
            timer.code.release(agent);
            return Err(queue_error(agent, error).into());
        }
        result
    }
}

fn clamp_delay(delay: Duration, nesting: u8) -> Duration {
    if nesting > 5 {
        delay.max(Duration::from_millis(4))
    } else {
        delay
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustic_js::runtime::Roots;

    #[test]
    fn queued_timer_retains_its_callback_and_arguments_through_collection() {
        let mut agent = Agent::new().unwrap();
        let callback = agent
            .evaluate("(function (object) { if (object.value !== 42) throw 'lost argument'; })")
            .unwrap();
        let callback_root = agent.root(callback).unwrap();
        let argument = agent.evaluate("({value: 42})").unwrap();
        let code =
            TimerCode::retain(&mut agent, TimerHandler::Function(callback, &[argument])).unwrap();
        assert!(agent.unroot(callback_root));
        agent.collect_garbage(Roots::default());
        code.run(&mut agent).unwrap();
        let TimerCode::Function { root, .. } = code else {
            unreachable!()
        };
        code.release(&mut agent);
        assert!(agent.rooted(root).is_none());
    }
}
