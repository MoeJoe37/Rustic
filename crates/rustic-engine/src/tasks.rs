//! WEB-01 immediate and timed script task sources.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};
use std::time::Duration;

use rustic_base::{Budget, DocumentId, NodeId};
use rustic_js::Source;

use crate::{InputEvent, ScriptQueueError, TaskLimits};

/// Opaque identity of a one-shot timer within its owning document.
///
/// Keeping an ID does not keep the document or timer alive.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TimerId {
    document: DocumentId,
    sequence: u64,
}

struct TimedScript {
    deadline: Duration,
    sequence: u64,
    source: Source,
}

impl TimedScript {
    fn key(&self) -> (Duration, u64) {
        (self.deadline, self.sequence)
    }
}

impl PartialEq for TimedScript {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for TimedScript {}

impl PartialOrd for TimedScript {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimedScript {
    fn cmp(&self, other: &Self) -> Ordering {
        // A max-heap should pop the earliest deadline and then the oldest timer.
        other.key().cmp(&self.key())
    }
}

pub(crate) enum Task {
    Script(Source),
    Input { target: NodeId, event: InputEvent },
}

impl Task {
    fn units(&self) -> usize {
        match self {
            Self::Script(source) => source.units().len(),
            Self::Input {
                event: InputEvent::KeyDown(key) | InputEvent::KeyUp(key),
                ..
            } => key
                .key
                .as_units()
                .len()
                .saturating_add(key.code.as_units().len()),
            Self::Input {
                event: InputEvent::Click { .. },
                ..
            } => 0,
        }
    }
}

pub(crate) struct ScriptQueue {
    owner: DocumentId,
    immediate: VecDeque<Task>,
    timers: BinaryHeap<TimedScript>,
    task_limit: usize,
    source_budget: Budget,
    next_timer: u64,
    prefer_timers: bool,
}

impl ScriptQueue {
    pub(crate) fn new(owner: DocumentId, limits: TaskLimits) -> Self {
        Self {
            owner,
            immediate: VecDeque::new(),
            timers: BinaryHeap::new(),
            task_limit: limits.pending_scripts,
            source_budget: Budget::new(limits.pending_source_units),
            next_timer: 0,
            prefer_timers: false,
        }
    }

    pub(crate) fn queue(&mut self, task: Task) -> Result<(), ScriptQueueError> {
        let units = task.units();
        self.check_admission(units)?;
        self.immediate
            .try_reserve(1)
            .map_err(|_| ScriptQueueError::Capacity)?;
        self.source_budget.charge(units).expect("checked admission");
        self.immediate.push_back(task);
        Ok(())
    }

    pub(crate) fn schedule(
        &mut self,
        deadline: Duration,
        source: Source,
    ) -> Result<TimerId, ScriptQueueError> {
        let units = source.units().len();
        self.check_admission(units)?;
        let sequence = self.next_timer;
        let next = sequence
            .checked_add(1)
            .ok_or(ScriptQueueError::TimerIdsExhausted)?;
        self.timers
            .try_reserve(1)
            .map_err(|_| ScriptQueueError::Capacity)?;
        self.source_budget.charge(units).expect("checked admission");
        self.timers.push(TimedScript {
            deadline,
            sequence,
            source,
        });
        self.next_timer = next;
        Ok(TimerId {
            document: self.owner,
            sequence,
        })
    }

    fn check_admission(&self, units: usize) -> Result<(), ScriptQueueError> {
        if self.immediate.len() + self.timers.len() == self.task_limit
            || units > self.source_budget.remaining()
        {
            return Err(ScriptQueueError::Limit);
        }
        Ok(())
    }

    pub(crate) fn cancel(&mut self, id: TimerId) -> bool {
        if id.document != self.owner {
            return false;
        }
        let mut removed_units = None;
        self.timers.retain(|timer| {
            if timer.sequence == id.sequence {
                removed_units = Some(timer.source.units().len());
                return false;
            }
            true
        });
        if let Some(units) = removed_units {
            self.source_budget.release(units).expect("charged timer");
        }
        removed_units.is_some()
    }

    pub(crate) fn delay(&self, now: Duration) -> Option<Duration> {
        self.timers
            .peek()
            .map(|timer| timer.deadline.saturating_sub(now))
    }

    pub(crate) fn pop_ready(&mut self, now: Duration) -> Option<Task> {
        let timer_ready = self
            .timers
            .peek()
            .is_some_and(|timer| timer.deadline <= now);
        let task = if timer_ready && (self.prefer_timers || self.immediate.is_empty()) {
            self.prefer_timers = false;
            Task::Script(self.timers.pop().expect("a ready timer").source)
        } else {
            let task = self.immediate.pop_front()?;
            self.prefer_timers = true;
            task
        };
        self.source_budget
            .release(task.units())
            .expect("charged task");
        Some(task)
    }

    pub(crate) fn clear(&mut self) {
        self.immediate = VecDeque::new();
        self.timers = BinaryHeap::new();
        self.source_budget = Budget::new(self.source_budget.limit());
    }
}
