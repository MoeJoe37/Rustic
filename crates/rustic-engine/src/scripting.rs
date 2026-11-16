//! WEB-01 document-owned script tasks and microtask checkpoints.

use std::fmt;
use std::time::Duration;

use rustic_base::{Clock, SystemClock};
use rustic_js::runtime::{Abrupt, Agent, DEFAULT_HEAP_BYTES};
use rustic_js::{CompileError, EvalError, ParseError, Source};

use crate::EngineDocument;
use crate::tasks::{ScriptQueue, TimerId};

/// Resource limits for one scripted document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskLimits {
    /// Maximum number of immediate and timed script tasks waiting to run.
    pub pending_scripts: usize,
    /// Maximum total UTF-16 source units in waiting tasks.
    pub pending_source_units: usize,
    /// VM safe points shared by a script and its microtask checkpoint.
    pub steps_per_turn: u64,
    /// Jobs that a single microtask checkpoint may run, including newly queued jobs.
    pub jobs_per_checkpoint: usize,
    /// JavaScript heap budget in bytes.
    pub heap_bytes: usize,
}

impl Default for TaskLimits {
    fn default() -> Self {
        Self {
            pending_scripts: 128,
            pending_source_units: 512 * 1024,
            steps_per_turn: 1_000_000,
            jobs_per_checkpoint: 4096,
            heap_bytes: DEFAULT_HEAP_BYTES,
        }
    }
}

/// Why a script task could not be queued.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptQueueError {
    /// Script execution has stopped.
    Stopped(ScriptStop),
    /// The pending task or source limit would be exceeded.
    Limit,
    /// The queue could not allocate a slot.
    Capacity,
    /// The requested delay exceeds the monotonic clock range.
    DeadlineOverflow,
    /// This document has exhausted its non-reusable timer identities.
    TimerIdsExhausted,
}

impl fmt::Display for ScriptQueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped(reason) => reason.fmt(f),
            Self::Limit => f.write_str("pending script limit reached"),
            Self::Capacity => f.write_str("cannot allocate a script task slot"),
            Self::DeadlineOverflow => f.write_str("script timer deadline exceeds the clock range"),
            Self::TimerIdsExhausted => f.write_str("script timer identity space exhausted"),
        }
    }
}

impl std::error::Error for ScriptQueueError {}

/// Terminal reason for stopping a document's script execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptStop {
    /// The document's shared cancellation token was set.
    DocumentCancelled,
    /// The VM safe-point budget was exhausted.
    StepLimit,
    /// The checkpoint still had jobs after reaching its job limit.
    JobLimit,
    /// The JavaScript heap budget was exhausted.
    OutOfMemory,
}

impl fmt::Display for ScriptStop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DocumentCancelled => "document cancelled",
            Self::StepLimit => "script turn exhausted its safe-point budget",
            Self::JobLimit => "microtask checkpoint exhausted its job budget",
            Self::OutOfMemory => "script agent exhausted its heap budget",
        })
    }
}

impl std::error::Error for ScriptStop {}

/// A script failure that still permits the microtask checkpoint and later turns.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptFailure {
    /// The source did not parse.
    Parse(ParseError),
    /// The parsed script could not be compiled.
    Compile(CompileError),
    /// A thrown value reached the top level. The host receives the failure category.
    UncaughtException,
}

/// The result of one script task and its completed microtask checkpoint.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptTurn {
    /// The script completed normally.
    Completed,
    /// The script failed and the checkpoint still ran.
    Failed(ScriptFailure),
}

/// A completed document with a single script agent and two script task sources.
///
/// The host explicitly queues scripts and pumps turns. The document and agent
/// move together, with the document's existing token shared by the VM.
pub struct ScriptedDocument<C = SystemClock> {
    document: EngineDocument,
    agent: Result<Agent, ScriptStop>,
    pending: ScriptQueue,
    clock: C,
    limits: TaskLimits,
}

impl EngineDocument {
    /// Give this document a script agent with the requested limits.
    ///
    /// The conversion consumes the inert document. If initialization fails, its
    /// scope is closed as when dropping any document owner.
    pub fn into_scripted(self, limits: TaskLimits) -> Result<ScriptedDocument, ScriptStop> {
        self.into_scripted_with_clock(limits, SystemClock::default())
    }

    /// Give this document a script agent and a host-selected monotonic clock.
    /// The clock governs timer eligibility. JavaScript reads civil time separately.
    pub fn into_scripted_with_clock<C: Clock>(
        self,
        limits: TaskLimits,
        clock: C,
    ) -> Result<ScriptedDocument<C>, ScriptStop> {
        if self.cancellation().is_cancelled() {
            return Err(ScriptStop::DocumentCancelled);
        }
        let mut agent =
            Agent::with_heap_limit(limits.heap_bytes).map_err(|_| ScriptStop::OutOfMemory)?;
        agent.set_cancellation(Some(self.cancellation().clone()));
        agent.set_can_block(false);
        let pending = ScriptQueue::new(self.parsed().document().id(), limits);
        Ok(ScriptedDocument {
            document: self,
            agent: Ok(agent),
            pending,
            clock,
            limits,
        })
    }
}

impl<C: Clock> ScriptedDocument<C> {
    /// Inspect the parsed document and its lifetime token.
    pub fn document(&self) -> &EngineDocument {
        &self.document
    }

    /// Queue a host-supplied script after checking both pending-work limits.
    pub fn queue_script(&mut self, source: Source) -> Result<(), ScriptQueueError> {
        self.check_live().map_err(ScriptQueueError::Stopped)?;
        self.pending.queue(source)
    }

    /// Access the host-selected timer clock, for example to advance a manual clock.
    pub fn clock(&self) -> &C {
        &self.clock
    }

    /// Queue a one-shot script timer that becomes eligible after `delay`.
    /// A zero delay makes it eligible on a later host turn.
    pub fn schedule_script(
        &mut self,
        delay: Duration,
        source: Source,
    ) -> Result<TimerId, ScriptQueueError> {
        self.check_live().map_err(ScriptQueueError::Stopped)?;
        let deadline = self
            .clock
            .now()
            .checked_add(delay)
            .ok_or(ScriptQueueError::DeadlineOverflow)?;
        self.pending.schedule(deadline, source)
    }

    /// Remove a pending timer and release its queue budget.
    /// Returns `false` for a foreign timer or one that has already run or been removed.
    pub fn cancel_timer(&mut self, timer: TimerId) -> Result<bool, ScriptStop> {
        self.check_live()?;
        Ok(self.pending.cancel(timer))
    }

    /// Delay until the earliest pending timer, or zero if already eligible.
    /// Returns `None` if no timers are pending.
    pub fn time_until_next_timer(&mut self) -> Result<Option<Duration>, ScriptStop> {
        self.check_live()?;
        Ok(self.pending.delay(self.clock.now()))
    }

    /// Run one eligible script and drain its promise jobs before a later task.
    ///
    /// A terminal failure discards pending scripts and releases the JS agent.
    /// Returns `Ok(None)` when there is no eligible task.
    pub fn run_next(&mut self) -> Result<Option<ScriptTurn>, ScriptStop> {
        self.check_live()?;
        let Some(source) = self.pending.pop_ready(self.clock.now()) else {
            return Ok(None);
        };

        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        agent.set_step_budget(Some(self.limits.steps_per_turn));
        let turn = match agent.evaluate_source(&source) {
            Ok(_) => ScriptTurn::Completed,
            Err(EvalError::Parse(error)) => ScriptTurn::Failed(ScriptFailure::Parse(error)),
            Err(EvalError::Compile(error)) => ScriptTurn::Failed(ScriptFailure::Compile(error)),
            Err(EvalError::Throw(_)) => ScriptTurn::Failed(ScriptFailure::UncaughtException),
            Err(EvalError::OutOfMemory) => return Err(self.stop(ScriptStop::OutOfMemory)),
            Err(EvalError::Cancelled) => return Err(self.stop_after_cancellation()),
            Err(EvalError::SourceTooLong) => unreachable!("Source already bounds its length"),
        };
        match agent.run_jobs_limited(self.limits.jobs_per_checkpoint) {
            Ok(true) => {
                // Release the rejection report's roots at the checkpoint.
                // Web rejection-event dispatch belongs to the later event layer.
                agent.take_rejection_events();
                self.check_live()?;
                Ok(Some(turn))
            }
            Ok(false) => Err(self.stop(ScriptStop::JobLimit)),
            Err(Abrupt::OutOfMemory) => Err(self.stop(ScriptStop::OutOfMemory)),
            Err(Abrupt::Cancelled) => Err(self.stop_after_cancellation()),
            Err(Abrupt::Throw(_)) => unreachable!("the job runner handles thrown values"),
        }
    }

    fn check_live(&mut self) -> Result<(), ScriptStop> {
        if self.document.cancellation().is_cancelled() {
            return Err(self.stop(ScriptStop::DocumentCancelled));
        }
        self.agent.as_ref().map(|_| ()).map_err(|reason| *reason)
    }

    fn stop_after_cancellation(&mut self) -> ScriptStop {
        if self.document.cancellation().is_cancelled() {
            self.stop(ScriptStop::DocumentCancelled)
        } else {
            self.stop(ScriptStop::StepLimit)
        }
    }

    fn stop(&mut self, reason: ScriptStop) -> ScriptStop {
        self.agent = Err(reason);
        self.pending.clear();
        reason
    }
}
