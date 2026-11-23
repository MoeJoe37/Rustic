//! Document task sources and their shared admission limits.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};
use std::time::Duration;

use rustic_base::{Budget, DocumentId, NodeId};
use rustic_js::Source;
use rustic_js::runtime::{Agent, HostRoot, JsResult, RejectionOperation};

use crate::window_tasks::WindowTimer;
use crate::{InputEvent, ScriptQueueError, TaskLimits};

/// Opaque identity of a one-shot timer within its owning document.
///
/// Keeping an ID does not keep the document or timer alive.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TimerId {
    document: DocumentId,
    sequence: u64,
}

#[derive(Debug)]
struct TimedTask {
    deadline: Duration,
    sequence: u64,
    task: Task,
}

impl TimedTask {
    fn key(&self) -> (Duration, u64) {
        (self.deadline, self.sequence)
    }
}

impl PartialEq for TimedTask {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for TimedTask {}

impl PartialOrd for TimedTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimedTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // A max-heap should pop the earliest deadline and then the oldest timer.
        other.key().cmp(&self.key())
    }
}

/// A bounded host completion already fetched and staged outside the VM.
/// The closure must only publish validated data; it must not execute script.
pub(crate) struct ResourceCompletion(Box<dyn FnOnce() -> bool>);

impl ResourceCompletion {
    pub(crate) fn new<E>(apply: impl FnOnce() -> Result<(), E> + 'static) -> Self {
        Self(Box::new(move || apply().is_ok()))
    }

    pub(crate) fn apply(self) -> bool {
        (self.0)()
    }
}

impl std::fmt::Debug for ResourceCompletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResourceCompletion(..)")
    }
}

#[derive(Debug)]
pub(crate) enum Task {
    Script(Source),
    Timer(WindowTimer),
    Input {
        target: NodeId,
        event: InputEvent,
    },
    ResourceCompletion(ResourceCompletion),
    /// HTML "notify about rejected promises": the tracker's events from one
    /// checkpoint, with each promise rooted until the task runs.
    PromiseRejections(Vec<(RejectionOperation, HostRoot)>),
}

impl Task {
    fn units(&self) -> usize {
        match self {
            Self::Script(source) => source.units().len(),
            Self::Timer(timer) => timer.units,
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
            }
            | Self::ResourceCompletion(_)
            | Self::PromiseRejections(_) => 0,
        }
    }
}

#[derive(Debug)]
pub(crate) struct ScriptQueue {
    owner: DocumentId,
    immediate: VecDeque<Task>,
    timers: BinaryHeap<TimedTask>,
    staged: Vec<TimedTask>,
    running_interval: Option<WindowTimer>,
    frames: VecDeque<(u32, HostRoot)>,
    pub(crate) active_nesting: u8,
    window_timer: i32,
    frame_callback: u32,
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
            staged: Vec::new(),
            running_interval: None,
            frames: VecDeque::new(),
            active_nesting: 0,
            window_timer: 0,
            frame_callback: 0,
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
        let sequence = self.push_timer(deadline, Task::Script(source), false)?;
        Ok(TimerId {
            document: self.owner,
            sequence,
        })
    }

    pub(crate) fn stage_timer(
        &mut self,
        timer: WindowTimer,
        delay: Duration,
    ) -> Result<(), ScriptQueueError> {
        self.push_timer(delay, Task::Timer(timer), true).map(|_| ())
    }

    fn push_timer(
        &mut self,
        delay_or_deadline: Duration,
        task: Task,
        staged: bool,
    ) -> Result<u64, ScriptQueueError> {
        let units = task.units();
        self.check_admission(units)?;
        let sequence = self.next_timer;
        let next = sequence
            .checked_add(1)
            .ok_or(ScriptQueueError::TimerIdsExhausted)?;
        // Reserve space for staged timers before committing admission.
        self.timers
            .try_reserve(self.staged.len().saturating_add(1))
            .map_err(|_| ScriptQueueError::Capacity)?;
        if staged {
            self.staged
                .try_reserve(1)
                .map_err(|_| ScriptQueueError::Capacity)?;
        }
        self.source_budget.charge(units).expect("checked admission");
        let timer = TimedTask {
            deadline: delay_or_deadline,
            sequence,
            task,
        };
        if staged {
            self.staged.push(timer);
        } else {
            self.timers.push(timer);
        }
        self.next_timer = next;
        Ok(sequence)
    }

    pub(crate) fn publish_timers(&mut self, now: Duration) {
        for mut timer in self.staged.drain(..) {
            timer.deadline = now.saturating_add(timer.deadline);
            self.timers.push(timer);
        }
    }

    pub(crate) fn start_interval(&mut self, timer: WindowTimer) {
        debug_assert!(self.running_interval.is_none());
        self.source_budget
            .charge(timer.units)
            .expect("due timer released its charge");
        self.running_interval = Some(timer);
    }

    pub(crate) fn take_interval(&mut self) -> Option<WindowTimer> {
        let timer = self.running_interval.take()?;
        self.source_budget
            .release(timer.units)
            .expect("charged interval");
        Some(timer)
    }

    pub(crate) fn check_admission(&self, units: usize) -> Result<(), ScriptQueueError> {
        let len = self.immediate.len()
            + self.timers.len()
            + self.staged.len()
            + self.frames.len()
            + usize::from(self.running_interval.is_some());
        if len == self.task_limit || units > self.source_budget.remaining() {
            return Err(ScriptQueueError::Limit);
        }
        Ok(())
    }

    pub(crate) fn cancel(&mut self, id: TimerId) -> bool {
        id.document == self.owner
            && self
                .remove_timer(|timer| timer.sequence == id.sequence)
                .is_some()
    }

    pub(crate) fn next_window_timer(&self) -> Result<i32, ScriptQueueError> {
        self.window_timer
            .checked_add(1)
            .ok_or(ScriptQueueError::TimerIdsExhausted)
    }

    pub(crate) fn commit_window_timer(&mut self, id: i32) {
        self.window_timer = id;
    }

    pub(crate) fn cancel_window_timer(
        &mut self,
        agent: &mut Agent,
        id: i32,
    ) -> JsResult<Option<WindowTimer>> {
        if id <= 0 || id > self.window_timer {
            return Ok(None);
        }
        if self.running_interval.is_some_and(|timer| timer.id == id) {
            return Ok(self.take_interval());
        }
        for _ in 0..self.staged.len() + self.timers.len() {
            agent.check_native_work()?;
        }
        Ok(self
            .remove_timer(|timer| matches!(timer.task, Task::Timer(callback) if callback.id == id))
            .map(|task| match task {
                Task::Timer(callback) => callback,
                _ => unreachable!("matched a Window timer"),
            }))
    }

    fn remove_timer(&mut self, matches: impl Fn(&TimedTask) -> bool) -> Option<Task> {
        if let Some(index) = self.staged.iter().position(&matches) {
            let task = self.staged.swap_remove(index).task;
            self.source_budget
                .release(task.units())
                .expect("charged timer");
            return Some(task);
        }
        // BinaryHeap cannot remove by index. Reuse its allocation and heapify
        // once after the swap instead of rebuilding by individual pushes.
        let mut timers = std::mem::take(&mut self.timers).into_vec();
        let removed = timers
            .iter()
            .position(matches)
            .map(|index| timers.swap_remove(index).task);
        self.timers = BinaryHeap::from(timers);
        if let Some(task) = &removed {
            self.source_budget
                .release(task.units())
                .expect("charged timer");
        }
        removed
    }

    pub(crate) fn reserve_frame(&mut self) -> Result<u32, ScriptQueueError> {
        self.check_admission(0)?;
        let id = self
            .frame_callback
            .checked_add(1)
            .ok_or(ScriptQueueError::AnimationIdsExhausted)?;
        self.frames
            .try_reserve(1)
            .map_err(|_| ScriptQueueError::Capacity)?;
        Ok(id)
    }

    pub(crate) fn queue_frame(&mut self, id: u32, callback: HostRoot) {
        self.frame_callback = id;
        self.frames.push_back((id, callback));
    }

    pub(crate) fn cancel_frame(
        &mut self,
        agent: &mut Agent,
        id: u32,
    ) -> JsResult<Option<HostRoot>> {
        if id == 0 || id > self.frame_callback {
            return Ok(None);
        }
        for (index, &(handle, _)) in self.frames.iter().enumerate() {
            agent.check_native_work()?;
            if handle == id {
                return Ok(self.frames.remove(index).map(|(_, callback)| callback));
            }
        }
        Ok(None)
    }

    pub(crate) fn has_frames(&self) -> bool {
        !self.frames.is_empty()
    }

    pub(crate) fn frame_cursor(&self) -> u32 {
        self.frame_callback
    }

    pub(crate) fn pop_frame(&mut self, cursor: u32) -> Option<HostRoot> {
        let &(id, _) = self.frames.front()?;
        if id > cursor {
            return None;
        }
        self.frames.pop_front().map(|(_, callback)| callback)
    }

    pub(crate) fn delay(&self, now: Duration) -> Option<Duration> {
        self.timers
            .peek()
            .map(|timer| timer.deadline.saturating_sub(now))
    }

    fn timer_has_priority(&self, now: Duration) -> bool {
        (self.prefer_timers || self.immediate.is_empty())
            && self
                .timers
                .peek()
                .is_some_and(|timer| timer.deadline <= now)
    }

    pub(crate) fn next_ready_is_resource(&self, now: Duration) -> bool {
        !self.timer_has_priority(now)
            && matches!(self.immediate.front(), Some(Task::ResourceCompletion(_)))
    }

    pub(crate) fn pop_ready(&mut self, now: Duration) -> Option<Task> {
        let task = if self.timer_has_priority(now) {
            self.prefer_timers = false;
            self.timers.pop().expect("a ready timer").task
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

    /// Remove the oldest queued rejection notification, which shares the DOM
    /// manipulation task source with the document's readiness steps.
    pub(crate) fn take_rejection_task(&mut self) -> Option<Task> {
        let index = self
            .immediate
            .iter()
            .position(|task| matches!(task, Task::PromiseRejections(_)))?;
        self.immediate.remove(index)
    }

    pub(crate) fn clear(&mut self) {
        self.immediate = VecDeque::new();
        self.timers = BinaryHeap::new();
        self.staged = Vec::new();
        self.running_interval = None;
        self.frames = VecDeque::new();
        self.source_budget = Budget::new(self.source_budget.limit());
    }
}
