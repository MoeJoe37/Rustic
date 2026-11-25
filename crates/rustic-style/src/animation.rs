//! The animation model: Web Animations timing and playback (Web
//! Animations Level 1 §§4–5 and Level 2 additions), CSS transitions (CSS
//! Transitions Level 1 §3 and Level 2 §§3–6) and CSS animations (CSS
//! Animations Level 1 §§3–4 and Level 2 §§3–5), editor's drafts reviewed
//! 2026-11-25.
//!
//! [`AnimationController`] is document state. The host advances it with
//! [`AnimationController::tick`] at each rendering opportunity (which runs
//! pending play and pause tasks, updates finished states and queues
//! events), then restyles with [`crate::style_tree_animated`], which starts,
//! updates and cancels CSS transitions and animations from the new styles
//! and samples every effect into the cascade's animation and transition
//! origins. Times are milliseconds on the document timeline, whose zero is
//! the host's time origin.
//!
//! Keyframe values are specified values; each is computed for its target
//! by running the cascade with the keyframe's declarations in the
//! animation origin, so `em`, `var()` and `currentcolor` resolve in the
//! element's context.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use rustic_css::{
    AnimationDirection, AnimationDuration, AnimationTimeline, AnimationValue, CompositeOperation,
    DeclaredValue, EasingFunction, FillMode, IterationCount, LengthPercentage,
    LengthPercentageOrAuto, PlayState, PropertyDeclaration, PropertyId, PropertyName,
    PropertyValue, RangeEdge, ScrollAxis, Scroller, TimelineRangeName, TransitionBehavior,
    TransitionProperty, ViewInset, cycle,
};

/// `lp` in pixels, with percentages of `basis` (font-relative units, which
/// a range offset should not use, count as pixels).
pub fn length_px(lp: &LengthPercentage, basis: f64) -> f64 {
    match lp {
        LengthPercentage::Length(l) => l.to_px().unwrap_or(l.value),
        LengthPercentage::Percentage(p) => basis * p / 100.0,
        LengthPercentage::Calc(c) => c
            .resolve(|l| l.to_px().unwrap_or(l.value), Some(basis))
            .unwrap_or(0.0),
    }
}
use rustic_dom::NodeId;

use crate::interpolate::{AnimationType, animation_type, composite, interpolable, interpolate};
use crate::{ComputedStyle, KeyframeSelector, KeyframesRule};

/// Identifies an animation for the life of its controller. Larger ids were
/// created later, which is the composite order of script animations.
pub type AnimationId = u64;

/// The fill mode of an effect, including Web Animations' `auto` (which
/// is `none` for keyframe effects).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fill {
    /// `none`.
    None,
    /// `forwards`.
    Forwards,
    /// `backwards`.
    Backwards,
    /// `both`.
    Both,
    /// `auto`.
    Auto,
}

impl Fill {
    fn backwards(self) -> bool {
        matches!(self, Self::Backwards | Self::Both)
    }

    fn forwards(self) -> bool {
        matches!(self, Self::Forwards | Self::Both)
    }

    /// The keyword.
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Forwards => "forwards",
            Self::Backwards => "backwards",
            Self::Both => "both",
            Self::Auto => "auto",
        }
    }
}

impl From<FillMode> for Fill {
    fn from(mode: FillMode) -> Self {
        match mode {
            FillMode::None => Self::None,
            FillMode::Forwards => Self::Forwards,
            FillMode::Backwards => Self::Backwards,
            FillMode::Both => Self::Both,
        }
    }
}

/// An effect's timing (Web Animations §4.6–4.8, the `EffectTiming`
/// dictionary). Times are milliseconds.
#[derive(Clone, Debug, PartialEq)]
pub struct Timing {
    /// Start delay.
    pub delay: f64,
    /// End delay.
    pub end_delay: f64,
    /// Fill mode.
    pub fill: Fill,
    /// Iteration start (a nonnegative number).
    pub iteration_start: f64,
    /// Iteration count (nonnegative, possibly infinite).
    pub iterations: f64,
    /// Iteration duration (nonnegative, possibly infinite).
    pub duration: f64,
    /// Playback direction.
    pub direction: AnimationDirection,
    /// The effect's timing function.
    pub easing: EasingFunction,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            delay: 0.0,
            end_delay: 0.0,
            fill: Fill::Auto,
            iteration_start: 0.0,
            iterations: 1.0,
            duration: 0.0,
            direction: AnimationDirection::Normal,
            easing: EasingFunction::LINEAR,
        }
    }
}

/// An effect's phase (Web Animations §4.8.3).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    /// No local time.
    Idle,
    /// Before the active interval.
    Before,
    /// In the active interval.
    Active,
    /// After the active interval.
    After,
}

/// An effect's computed timing at one local time (the
/// `ComputedEffectTiming` dictionary).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ComputedTiming {
    /// The phase.
    pub phase: Phase,
    /// The local time, if any.
    pub local_time: Option<f64>,
    /// The active time, if the effect is in effect.
    pub active_time: Option<f64>,
    /// The overall progress.
    pub overall_progress: Option<f64>,
    /// The simple iteration progress.
    pub simple_progress: Option<f64>,
    /// The current iteration.
    pub current_iteration: Option<f64>,
    /// The transformed (eased) progress.
    pub progress: Option<f64>,
    /// The active duration.
    pub active_duration: f64,
    /// The end time.
    pub end_time: f64,
}

impl Timing {
    /// The active duration: duration × iterations (zero when either is).
    pub fn active_duration(&self) -> f64 {
        if self.duration == 0.0 || self.iterations == 0.0 {
            0.0
        } else {
            self.duration * self.iterations
        }
    }

    /// The end time: delay + active duration + end delay, at least zero.
    pub fn end_time(&self) -> f64 {
        (self.delay + self.active_duration() + self.end_delay).max(0.0)
    }

    /// The computed timing at `local_time`; `backwards` when the
    /// animation's playback rate is negative.
    pub fn compute(&self, local_time: Option<f64>, backwards: bool) -> ComputedTiming {
        let active_duration = self.active_duration();
        let end_time = self.end_time();
        let mut out = ComputedTiming {
            phase: Phase::Idle,
            local_time,
            active_time: None,
            overall_progress: None,
            simple_progress: None,
            current_iteration: None,
            progress: None,
            active_duration,
            end_time,
        };
        let Some(local) = local_time else {
            return out;
        };
        let before_active = self.delay.min(end_time).max(0.0);
        let active_after = (self.delay + active_duration).min(end_time).max(0.0);
        out.phase = if local < before_active || (backwards && local == before_active) {
            Phase::Before
        } else if local > active_after || (!backwards && local == active_after) {
            Phase::After
        } else {
            Phase::Active
        };
        let fill = match self.fill {
            Fill::Auto => Fill::None,
            other => other,
        };
        let active_time = match out.phase {
            Phase::Before if fill.backwards() => Some((local - self.delay).max(0.0)),
            Phase::Active => Some(local - self.delay),
            Phase::After if fill.forwards() => {
                Some((local - self.delay).min(active_duration).max(0.0))
            }
            _ => None,
        };
        out.active_time = active_time;
        let Some(active) = active_time else {
            return out;
        };
        let overall = if self.duration == 0.0 {
            if out.phase == Phase::Before {
                self.iteration_start
            } else {
                self.iteration_start + self.iterations
            }
        } else {
            active / self.duration + self.iteration_start
        };
        out.overall_progress = Some(overall);
        let mut simple = if overall.is_infinite() {
            self.iteration_start % 1.0
        } else {
            overall % 1.0
        };
        if simple == 0.0
            && matches!(out.phase, Phase::Active | Phase::After)
            && active == active_duration
            && self.iterations != 0.0
        {
            simple = 1.0;
        }
        out.simple_progress = Some(simple);
        let iteration = if out.phase == Phase::After && self.iterations.is_infinite() {
            f64::INFINITY
        } else if simple == 1.0 {
            overall.floor() - 1.0
        } else {
            overall.floor()
        };
        out.current_iteration = Some(iteration);
        let forwards = match self.direction {
            AnimationDirection::Normal => true,
            AnimationDirection::Reverse => false,
            AnimationDirection::Alternate => iteration % 2.0 == 0.0 || iteration.is_infinite(),
            AnimationDirection::AlternateReverse => {
                !(iteration % 2.0 == 0.0 || iteration.is_infinite())
            }
        };
        let directed = if forwards { simple } else { 1.0 - simple };
        let before_flag = (out.phase == Phase::Before && forwards != backwards)
            || (out.phase == Phase::After && forwards == backwards);
        out.progress = Some(self.easing.sample(directed, before_flag));
        out
    }
}

/// Which values a keyframe holds.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyframeValues {
    /// Specified declarations, computed for the target when sampled.
    Declarations(Vec<PropertyDeclaration>),
    /// Computed values (transitions).
    Computed(Vec<(PropertyId, PropertyValue)>),
}

/// One keyframe of an effect.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectKeyframe {
    /// The specified offset, if any.
    pub offset: Option<f64>,
    /// The computed offset (specified, or distributed evenly).
    pub computed_offset: f64,
    /// The timing function from this keyframe to the next.
    pub easing: EasingFunction,
    /// The keyframe's composite operation; `None` uses the effect's.
    pub composite: Option<CompositeOperation>,
    /// The values.
    pub values: KeyframeValues,
}

impl Default for EffectKeyframe {
    /// An empty, linear keyframe without an offset.
    fn default() -> Self {
        Self {
            offset: None,
            computed_offset: 0.0,
            easing: EasingFunction::LINEAR,
            composite: None,
            values: KeyframeValues::Declarations(Vec::new()),
        }
    }
}

/// Fill in computed offsets (Web Animations §5.3.3 "compute missing
/// keyframe offsets").
pub fn distribute_offsets(keyframes: &mut [EffectKeyframe]) {
    let len = keyframes.len();
    if len == 0 {
        return;
    }
    for (i, k) in keyframes.iter_mut().enumerate() {
        k.computed_offset = k.offset.unwrap_or(if i == 0 {
            0.0
        } else if i == len - 1 && len > 1 {
            1.0
        } else {
            f64::NAN
        });
    }
    if len == 1 && keyframes[0].offset.is_none() {
        keyframes[0].computed_offset = 1.0;
    }
    let mut i = 0;
    while i < len {
        if !keyframes[i].computed_offset.is_nan() {
            i += 1;
            continue;
        }
        let start = i - 1;
        let mut end = i;
        while keyframes[end].computed_offset.is_nan() {
            end += 1;
        }
        let (a, b) = (
            keyframes[start].computed_offset,
            keyframes[end].computed_offset,
        );
        let gap = (end - start) as f64;
        for (step, k) in keyframes[start + 1..end].iter_mut().enumerate() {
            k.computed_offset = a + (b - a) * (step + 1) as f64 / gap;
        }
        i = end;
    }
}

/// What an effect animates.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyframeEffect {
    /// The target element (pseudo-elements are not supported, TD-202).
    pub target: Option<NodeId>,
    /// The keyframes, sorted by computed offset (stable).
    pub keyframes: Vec<EffectKeyframe>,
    /// The effect's composite operation.
    pub composite: CompositeOperation,
    /// The timing.
    pub timing: Timing,
}

impl KeyframeEffect {
    /// An effect with `keyframes`, distributing missing offsets.
    pub fn new(target: Option<NodeId>, mut keyframes: Vec<EffectKeyframe>, timing: Timing) -> Self {
        distribute_offsets(&mut keyframes);
        Self {
            target,
            keyframes,
            composite: CompositeOperation::Replace,
            timing,
        }
    }

    /// The longhands any keyframe sets, in [`PropertyId::ALL`] order.
    pub fn properties(&self) -> Vec<PropertyId> {
        let mut seen = HashSet::new();
        for k in &self.keyframes {
            match &k.values {
                KeyframeValues::Declarations(d) => {
                    seen.extend(d.iter().filter_map(|d| d.property.longhand()));
                }
                KeyframeValues::Computed(v) => seen.extend(v.iter().map(|(p, _)| *p)),
            }
        }
        PropertyId::ALL
            .iter()
            .copied()
            .filter(|p| seen.contains(p) && animation_type(*p) != AnimationType::NotAnimatable)
            .collect()
    }
}

/// Who created an animation.
#[derive(Clone, Debug, PartialEq)]
pub enum Owner {
    /// Script (`element.animate()` or the `Animation` constructor).
    Script,
    /// A CSS animation: its element, `animation-name` and list position.
    CssAnimation {
        /// The element.
        element: NodeId,
        /// The keyframes name.
        name: Arc<str>,
        /// Its index in `animation-name`.
        index: usize,
    },
    /// A CSS transition.
    CssTransition {
        /// The element.
        element: NodeId,
        /// The transitioning longhand.
        property: PropertyId,
        /// Generation, for composite order among transitions.
        generation: u64,
    },
}

/// An animation's play state (Web Animations §4.4.15).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlayStateValue {
    /// `idle`.
    Idle,
    /// `running`.
    Running,
    /// `paused`.
    Paused,
    /// `finished`.
    Finished,
}

impl PlayStateValue {
    /// The keyword.
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Finished => "finished",
        }
    }
}

/// An animation's replace state (Web Animations §5.5).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplaceState {
    /// `active`.
    Active,
    /// `removed`.
    Removed,
    /// `persisted`.
    Persisted,
}

impl ReplaceState {
    /// The keyword.
    pub fn name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Removed => "removed",
            Self::Persisted => "persisted",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pending {
    Play,
    Pause,
}

/// Which timeline drives an animation.
#[derive(Clone, Debug, PartialEq)]
pub enum TimelineRef {
    /// The document timeline.
    Document,
    /// No timeline (the animation cannot play).
    None,
    /// A progress-based scroll or view timeline (see
    /// [`AnimationController::set_scroll_timeline`]); its time is a
    /// percentage in `0..=100` of the animation's attachment range.
    Scroll(ScrollRef),
}

/// An animation's attachment to a scroll or view timeline: the timeline
/// and the `animation-range` it maps to `0..=100`.
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollRef {
    /// The timeline (see [`AnimationController::scroll_timelines`]).
    pub id: u64,
    /// `animation-range-start`.
    pub start: RangeEdge,
    /// `animation-range-end`.
    pub end: RangeEdge,
}

impl ScrollRef {
    /// The whole timeline.
    pub fn whole(id: u64) -> Self {
        Self {
            id,
            start: RangeEdge::NORMAL,
            end: RangeEdge::NORMAL,
        }
    }
}

/// The scroll container of a scroll progress timeline.
#[derive(Clone, Debug, PartialEq)]
pub enum ScrollerRef {
    /// This element (`scroll(self)` or a named `scroll-timeline`).
    Element(NodeId),
    /// The nearest ancestor scroll container of this element
    /// (`scroll(nearest)`), or the viewport.
    NearestTo(NodeId),
    /// The viewport (`scroll(root)`).
    Root,
}

/// What a scroll or view timeline measures; the host computes its state
/// from layout and scroll positions.
#[derive(Clone, Debug, PartialEq)]
pub enum TimelineSource {
    /// A scroll progress timeline.
    Scroll {
        /// The scroll container.
        scroller: ScrollerRef,
        /// The axis.
        axis: ScrollAxis,
    },
    /// A view progress timeline of `subject` in its nearest ancestor
    /// scroll container.
    View {
        /// The subject element.
        subject: NodeId,
        /// The axis.
        axis: ScrollAxis,
        /// `view-timeline-inset`.
        inset: ViewInset,
    },
}

/// Why an animation operation failed (a `DOMException` name).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnimationError {
    /// `InvalidStateError`.
    InvalidState,
}

/// An animation (Web Animations §4.4).
#[derive(Clone, Debug)]
pub struct Animation {
    /// Its id.
    pub id: AnimationId,
    /// Who created it.
    pub owner: Owner,
    /// Its effect.
    pub effect: Option<KeyframeEffect>,
    /// Its timeline.
    pub timeline: TimelineRef,
    start_time: Option<f64>,
    hold_time: Option<f64>,
    playback_rate: f64,
    pending_rate: Option<f64>,
    pending: Option<Pending>,
    previous_current_time: Option<f64>,
    finished_notified: bool,
    replace_state: ReplaceState,
    /// The phase and iteration the last tick saw (for CSS events).
    last_phase: Phase,
    last_iteration: Option<f64>,
    /// Whether a CSS animation is paused by `animation-play-state` (or
    /// script took over its play state).
    css_paused: bool,
    script_play_state: bool,
    /// Transition data: the start and end values, the reversing-adjusted
    /// start value and the shortening factor.
    transition: Option<TransitionData>,
}

#[derive(Clone, Debug)]
struct TransitionData {
    property: PropertyId,
    from: PropertyValue,
    to: PropertyValue,
    adjusted_start: PropertyValue,
    shortening: f64,
}

impl Animation {
    fn new(
        id: AnimationId,
        owner: Owner,
        effect: Option<KeyframeEffect>,
        timeline: TimelineRef,
    ) -> Self {
        Self {
            id,
            owner,
            effect,
            timeline,
            start_time: None,
            hold_time: None,
            playback_rate: 1.0,
            pending_rate: None,
            pending: None,
            previous_current_time: None,
            finished_notified: false,
            replace_state: ReplaceState::Active,
            last_phase: Phase::Idle,
            last_iteration: None,
            css_paused: false,
            script_play_state: false,
            transition: None,
        }
    }

    /// The start time, if resolved.
    pub fn start_time(&self) -> Option<f64> {
        self.start_time
    }

    /// The playback rate.
    pub fn playback_rate(&self) -> f64 {
        self.playback_rate
    }

    /// Whether a play or pause task is pending.
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }

    /// The replace state.
    pub fn replace_state(&self) -> ReplaceState {
        self.replace_state
    }

    fn effective_rate(&self) -> f64 {
        self.pending_rate.unwrap_or(self.playback_rate)
    }

    fn effect_end(&self) -> f64 {
        self.effect.as_ref().map_or(0.0, |e| e.timing.end_time())
    }
}

/// What an animation event is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnimationEventKind {
    /// `transitionrun`.
    TransitionRun,
    /// `transitionstart`.
    TransitionStart,
    /// `transitionend`.
    TransitionEnd,
    /// `transitioncancel`.
    TransitionCancel,
    /// `animationstart`.
    AnimationStart,
    /// `animationiteration`.
    AnimationIteration,
    /// `animationend`.
    AnimationEnd,
    /// `animationcancel`.
    AnimationCancel,
    /// The animation finished (`finish` event, `finished` promise).
    Finish,
    /// The animation was cancelled (`cancel` event, promises rejected).
    Cancel,
    /// The animation was removed as replaced (`remove` event).
    Remove,
    /// The animation's pending task ran (`ready` promise).
    Ready,
    /// The animation stopped being finished (a new `finished` promise).
    Unfinish,
}

impl AnimationEventKind {
    /// The DOM event type, for the events that are DOM events.
    pub fn event_type(self) -> &'static str {
        match self {
            Self::TransitionRun => "transitionrun",
            Self::TransitionStart => "transitionstart",
            Self::TransitionEnd => "transitionend",
            Self::TransitionCancel => "transitioncancel",
            Self::AnimationStart => "animationstart",
            Self::AnimationIteration => "animationiteration",
            Self::AnimationEnd => "animationend",
            Self::AnimationCancel => "animationcancel",
            Self::Finish => "finish",
            Self::Cancel => "cancel",
            Self::Remove => "remove",
            Self::Ready => "ready",
            Self::Unfinish => "unfinish",
        }
    }
}

/// An event the host dispatches (or a promise it settles).
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationEvent {
    /// What happened.
    pub kind: AnimationEventKind,
    /// The animation.
    pub animation: AnimationId,
    /// The element, for CSS events.
    pub target: Option<NodeId>,
    /// The property (transitions) or keyframes name (animations).
    pub name: String,
    /// `elapsedTime` in seconds (CSS events).
    pub elapsed_time: f64,
    /// The animation's current time (playback events), milliseconds.
    pub current_time: Option<f64>,
    /// The timeline's time (playback events), milliseconds.
    pub timeline_time: Option<f64>,
}

#[derive(Debug, Default)]
struct ElementState {
    /// The after-change style of the last style change event.
    before_change: Option<ComputedStyle>,
    /// Running transitions by property.
    transitions: HashMap<PropertyId, AnimationId>,
    /// End values of completed transitions.
    completed: HashMap<PropertyId, PropertyValue>,
    /// CSS animations in `animation-name` order.
    css_animations: Vec<AnimationId>,
}

/// A scroll or view timeline's current state, set by layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollTimelineState {
    /// The current scroll offset along the axis, or `None` when the
    /// timeline is inactive (no scroll container or no scrollable range).
    pub offset: Option<f64>,
    /// The scroll offsets at 0% and 100% (a view timeline's cover range).
    pub start: f64,
    /// See [`Self::start`].
    pub end: f64,
    /// For a view timeline: the subject's start position in the scroll
    /// container's content and its size, and the scrollport size, along
    /// the axis.
    pub view: Option<ViewGeometry>,
}

/// A view timeline's geometry along its axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewGeometry {
    /// The subject's start edge in the scroll container's content.
    pub subject_start: f64,
    /// The subject's size.
    pub subject_size: f64,
    /// The scrollport's size (less the view timeline insets).
    pub port: f64,
}

impl ScrollTimelineState {
    /// The progress as a percentage of the whole timeline.
    pub fn progress(&self) -> Option<f64> {
        let offset = self.offset?;
        (self.end > self.start).then(|| (offset - self.start) / (self.end - self.start) * 100.0)
    }

    /// The scroll offsets of a named range (Scroll-driven Animations §4.1).
    fn named(&self, name: TimelineRangeName) -> (f64, f64) {
        let Some(v) = self.view else {
            return (self.start, self.end);
        };
        let (p, h, s) = (v.subject_start, v.subject_size, v.port);
        let contain = if h <= s {
            (p + h - s, p)
        } else {
            (p, p + h - s)
        };
        match name {
            TimelineRangeName::Cover | TimelineRangeName::Scroll => (self.start, self.end),
            TimelineRangeName::Contain => contain,
            TimelineRangeName::Entry => (p - s, contain.0),
            TimelineRangeName::Exit => (contain.1, p + h),
            TimelineRangeName::EntryCrossing => (p - s, p - s + h),
            TimelineRangeName::ExitCrossing => (p, p + h),
        }
    }

    /// The percentage of the whole timeline a range edge is at
    /// (`normal` is 0% for a start and 100% for an end).
    pub fn edge(&self, edge: &RangeEdge, is_end: bool) -> f64 {
        let (from, to) = match edge.name {
            Some(name) => self.named(name),
            None => (self.start, self.end),
        };
        let offset = match &edge.offset {
            None => {
                if edge.name.is_none() {
                    return if is_end { 100.0 } else { 0.0 };
                }
                if is_end { to } else { from }
            }
            Some(lp) => from + length_px(lp, to - from),
        };
        if self.end > self.start {
            (offset - self.start) / (self.end - self.start) * 100.0
        } else {
            0.0
        }
    }
}

/// One `@keyframes` frame: offset, easing, composite and declarations.
type RawFrame = (
    f64,
    Option<EasingFunction>,
    Option<CompositeOperation>,
    Vec<PropertyDeclaration>,
);

/// Document animation state; see the module documentation.
#[derive(Debug, Default)]
pub struct AnimationController {
    now: Option<f64>,
    animations: BTreeMap<AnimationId, Animation>,
    elements: HashMap<NodeId, ElementState>,
    next_id: AnimationId,
    transition_generation: u64,
    events: Vec<AnimationEvent>,
    seen: HashSet<NodeId>,
    scroll_timelines: HashMap<u64, ScrollTimelineState>,
    last_transition_progress: Option<f64>,
    /// Timelines that CSS animations use, by id.
    timeline_sources: BTreeMap<u64, TimelineSource>,
    next_timeline: u64,
    /// Timelines used by the restyle in progress.
    used_timelines: HashSet<u64>,
    /// Named timelines declared so far in the restyle in progress: the
    /// declaring element, the name, and the source.
    declared_timelines: Vec<(NodeId, Arc<str>, TimelineSource)>,
    /// The ancestors of the element being styled, outermost first.
    ancestors: Vec<NodeId>,
}

/// The values one element gets from its animations: declarations for the
/// cascade's animation origin and transition origin.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimatedValues {
    /// Animation-origin declarations (CSS and script animations).
    pub animations: Vec<PropertyDeclaration>,
    /// Transition-origin declarations.
    pub transitions: Vec<PropertyDeclaration>,
}

fn declaration(property: PropertyId, value: PropertyValue) -> PropertyDeclaration {
    PropertyDeclaration {
        property: PropertyName::Longhand(property),
        value: DeclaredValue::Value(value),
        important: false,
    }
}

impl AnimationController {
    /// An empty controller whose timeline has not started.
    pub fn new() -> Self {
        Self::default()
    }

    /// The document timeline's current time, once ticked.
    pub fn now(&self) -> Option<f64> {
        self.now
    }

    fn timeline_time(&self, timeline: &TimelineRef) -> Option<f64> {
        match timeline {
            TimelineRef::Document => self.now,
            TimelineRef::None => None,
            TimelineRef::Scroll(r) => {
                let state = self.scroll_timelines.get(&r.id)?;
                let progress = state.progress()?;
                let start = state.edge(&r.start, false);
                let end = state.edge(&r.end, true);
                (end > start).then(|| (progress - start) / (end - start) * 100.0)
            }
        }
    }

    /// The scroll and view timelines CSS animations use, for the host to
    /// measure before each tick with [`Self::set_scroll_timeline`].
    pub fn scroll_timelines(&self) -> impl Iterator<Item = (u64, &TimelineSource)> {
        self.timeline_sources.iter().map(|(id, s)| (*id, s))
    }

    /// The state last set for timeline `id`.
    pub fn scroll_timeline(&self, id: u64) -> Option<&ScrollTimelineState> {
        self.scroll_timelines.get(&id)
    }

    /// The timeline id for `source`, registering it when new. Script
    /// timelines (`ScrollTimeline`, `ViewTimeline`) use this too.
    pub fn timeline_for(&mut self, source: TimelineSource) -> u64 {
        if let Some((&id, _)) = self.timeline_sources.iter().find(|(_, s)| **s == source) {
            self.used_timelines.insert(id);
            return id;
        }
        self.next_timeline += 1;
        let id = self.next_timeline;
        self.timeline_sources.insert(id, source);
        self.used_timelines.insert(id);
        id
    }

    /// Set the ancestors (outermost first) of the element the next
    /// [`Self::style_element`] styles; named timelines resolve through them.
    pub fn set_ancestors(&mut self, ancestors: impl IntoIterator<Item = NodeId>) {
        self.ancestors.clear();
        self.ancestors.extend(ancestors);
    }

    /// Record the named timelines `node` declares, from its style.
    fn declare_timelines(&mut self, node: NodeId, style: &ComputedStyle) {
        let list = |p| match style.get(p) {
            PropertyValue::Animation(v) => Some(v.clone()),
            _ => None,
        };
        for view in [false, true] {
            let (name, axis) = if view {
                (PropertyId::ViewTimelineName, PropertyId::ViewTimelineAxis)
            } else {
                (
                    PropertyId::ScrollTimelineName,
                    PropertyId::ScrollTimelineAxis,
                )
            };
            let Some(AnimationValue::TimelineNames(names)) = list(name) else {
                continue;
            };
            let axes = match list(axis) {
                Some(AnimationValue::Axes(axes)) => axes,
                _ => Vec::new(),
            };
            let insets = match list(PropertyId::ViewTimelineInset) {
                Some(AnimationValue::ViewInsets(insets)) => insets,
                _ => Vec::new(),
            };
            for (index, name) in names.iter().enumerate() {
                let Some(name) = &name.0 else {
                    continue;
                };
                let axis = cycle(&axes, index).copied().unwrap_or(ScrollAxis::Block);
                let source = if view {
                    TimelineSource::View {
                        subject: node,
                        axis,
                        inset: cycle(&insets, index).cloned().unwrap_or(ViewInset(
                            LengthPercentageOrAuto::Auto,
                            LengthPercentageOrAuto::Auto,
                        )),
                    }
                } else {
                    TimelineSource::Scroll {
                        scroller: ScrollerRef::Element(node),
                        axis,
                    }
                };
                self.declared_timelines.push((node, name.clone(), source));
            }
        }
    }

    /// The timeline for `animation-timeline` entry `value` of `node`.
    fn css_timeline(
        &mut self,
        node: NodeId,
        value: Option<&AnimationTimeline>,
        start: RangeEdge,
        end: RangeEdge,
    ) -> TimelineRef {
        let source = match value {
            None | Some(AnimationTimeline::Auto) => return TimelineRef::Document,
            Some(AnimationTimeline::None) => return TimelineRef::None,
            Some(AnimationTimeline::Named(name)) => {
                // The nearest declaration on the element or an ancestor;
                // later declarations on one element win.
                let found = std::iter::once(node)
                    .chain(self.ancestors.iter().rev().copied())
                    .find_map(|owner| {
                        self.declared_timelines
                            .iter()
                            .rev()
                            .find(|(n, declared, _)| *n == owner && declared == name)
                    });
                match found {
                    Some((_, _, source)) => source.clone(),
                    None => return TimelineRef::None,
                }
            }
            Some(AnimationTimeline::Scroll(scroller, axis)) => TimelineSource::Scroll {
                scroller: match scroller {
                    Scroller::Nearest => ScrollerRef::NearestTo(node),
                    Scroller::Root => ScrollerRef::Root,
                    Scroller::SelfElement => ScrollerRef::Element(node),
                },
                axis: *axis,
            },
            Some(AnimationTimeline::View(axis, inset_start, inset_end)) => TimelineSource::View {
                subject: node,
                axis: *axis,
                inset: ViewInset(inset_start.clone(), inset_end.clone()),
            },
        };
        TimelineRef::Scroll(ScrollRef {
            id: self.timeline_for(source),
            start,
            end,
        })
    }

    /// Record the state of a scroll or view timeline (layout calls this
    /// before each tick).
    pub fn set_scroll_timeline(&mut self, id: u64, state: ScrollTimelineState) {
        self.scroll_timelines.insert(id, state);
    }

    /// The animation `id`.
    pub fn get(&self, id: AnimationId) -> Option<&Animation> {
        self.animations.get(&id)
    }

    /// Every animation in composite order (creation order).
    pub fn animations(&self) -> impl Iterator<Item = &Animation> {
        self.animations.values()
    }

    /// Take the queued events, oldest first.
    pub fn take_events(&mut self) -> Vec<AnimationEvent> {
        std::mem::take(&mut self.events)
    }

    /// Whether any animation still needs frames: one with a pending task,
    /// or one whose effect is running on an active timeline.
    pub fn wants_frames(&self) -> bool {
        !self.events.is_empty()
            || self.animations.values().any(|a| {
                a.pending.is_some()
                    || (self.play_state(a.id) == Some(PlayStateValue::Running)
                        && a.timeline == TimelineRef::Document)
            })
    }

    // ----- Web Animations playback (Web Animations §4.4) -----

    /// Create an animation (`new Animation()`, `element.animate()`): idle
    /// until played.
    pub fn create(&mut self, effect: Option<KeyframeEffect>, timeline: TimelineRef) -> AnimationId {
        self.create_owned(Owner::Script, effect, timeline)
    }

    fn create_owned(
        &mut self,
        owner: Owner,
        effect: Option<KeyframeEffect>,
        timeline: TimelineRef,
    ) -> AnimationId {
        self.next_id += 1;
        let id = self.next_id;
        self.animations
            .insert(id, Animation::new(id, owner, effect, timeline));
        id
    }

    /// The current time of animation `id` (§4.4.4).
    pub fn current_time(&self, id: AnimationId) -> Option<f64> {
        let a = self.animations.get(&id)?;
        self.current_time_of(a)
    }

    fn current_time_of(&self, a: &Animation) -> Option<f64> {
        if let Some(hold) = a.hold_time {
            return Some(hold);
        }
        let timeline = self.timeline_time(&a.timeline)?;
        let start = a.start_time?;
        Some((timeline - start) * a.playback_rate)
    }

    /// The play state (§4.4.15).
    pub fn play_state(&self, id: AnimationId) -> Option<PlayStateValue> {
        let a = self.animations.get(&id)?;
        let current = self.current_time_of(a);
        Some(if current.is_none() && a.pending.is_none() {
            PlayStateValue::Idle
        } else if a.pending == Some(Pending::Pause)
            || (a.start_time.is_none() && a.pending != Some(Pending::Play))
        {
            PlayStateValue::Paused
        } else if let Some(t) = current.filter(|_| {
            let rate = a.effective_rate();
            rate != 0.0
        }) {
            let rate = a.effective_rate();
            if (rate > 0.0 && t >= a.effect_end()) || (rate < 0.0 && t <= 0.0) {
                PlayStateValue::Finished
            } else {
                PlayStateValue::Running
            }
        } else {
            PlayStateValue::Running
        })
    }

    /// The effect's computed timing now.
    pub fn computed_timing(&self, id: AnimationId) -> Option<ComputedTiming> {
        let a = self.animations.get(&id)?;
        let effect = a.effect.as_ref()?;
        Some(
            effect
                .timing
                .compute(self.current_time_of(a), a.playback_rate < 0.0),
        )
    }

    fn queue(&mut self, kind: AnimationEventKind, id: AnimationId) {
        let (current, timeline) = match self.animations.get(&id) {
            Some(a) => (self.current_time_of(a), self.timeline_time(&a.timeline)),
            None => (None, None),
        };
        self.events.push(AnimationEvent {
            kind,
            animation: id,
            target: None,
            name: String::new(),
            elapsed_time: 0.0,
            current_time: current,
            timeline_time: timeline,
        });
    }

    fn update_finished(&mut self, id: AnimationId, did_seek: bool) {
        let Some(a) = self.animations.get(&id) else {
            return;
        };
        let timeline = self.timeline_time(&a.timeline);
        let end = a.effect_end();
        let rate = a.playback_rate;
        let unconstrained = match (a.hold_time, did_seek) {
            (Some(hold), true) => Some(hold),
            _ => match (timeline, a.start_time) {
                (Some(t), Some(s)) => Some((t - s) * rate),
                _ => a.hold_time,
            },
        };
        let (mut hold, mut start) = (a.hold_time, a.start_time);
        if a.start_time.is_some() && a.pending != Some(Pending::Play) {
            if let Some(u) = unconstrained {
                if rate > 0.0 && u >= end {
                    hold = Some(if did_seek {
                        u
                    } else {
                        a.previous_current_time.map_or(end, |p| p.max(end))
                    });
                } else if rate < 0.0 && u <= 0.0 {
                    hold = Some(if did_seek {
                        u
                    } else {
                        a.previous_current_time.map_or(0.0, |p| p.min(0.0))
                    });
                } else if rate != 0.0 {
                    if let Some(t) = timeline {
                        if did_seek {
                            if let Some(h) = hold {
                                start = Some(t - h / rate);
                            }
                        }
                        hold = None;
                    }
                }
            }
        }
        let a = self.animations.get_mut(&id).expect("present");
        a.hold_time = hold;
        a.start_time = start;
        let current = match (a.hold_time, timeline, a.start_time) {
            (Some(h), ..) => Some(h),
            (None, Some(t), Some(s)) => Some((t - s) * a.playback_rate),
            _ => None,
        };
        a.previous_current_time = current;
        let finished = self.play_state(id) == Some(PlayStateValue::Finished);
        let a = self.animations.get_mut(&id).expect("present");
        if finished && !a.finished_notified {
            a.finished_notified = true;
            self.queue(AnimationEventKind::Finish, id);
        } else if !finished && a.finished_notified {
            a.finished_notified = false;
            self.queue(AnimationEventKind::Unfinish, id);
        }
    }

    /// `play()` (§4.4.10).
    pub fn play(&mut self, id: AnimationId, auto_rewind: bool) -> Result<(), AnimationError> {
        let current = self.current_time(id);
        let Some(a) = self.animations.get_mut(&id) else {
            return Ok(());
        };
        a.script_play_state = true;
        let aborted_pause = a.pending == Some(Pending::Pause);
        let rate = a.effective_rate();
        let end = a.effect_end();
        let mut seek = None;
        if rate > 0.0 && auto_rewind && current.is_none_or(|t| t < 0.0 || t >= end) {
            seek = Some(0.0);
        } else if rate < 0.0 && auto_rewind && current.is_none_or(|t| t <= 0.0 || t > end) {
            if end.is_infinite() {
                return Err(AnimationError::InvalidState);
            }
            seek = Some(end);
        } else if rate == 0.0 && current.is_none() {
            seek = Some(0.0);
        }
        if let Some(seek) = seek {
            a.start_time = None;
            a.hold_time = Some(seek);
        }
        if a.hold_time.is_some() {
            a.start_time = None;
        }
        let has_pending_rate = a.pending_rate.is_some();
        if a.pending.is_some() {
            a.pending = None;
        }
        if a.hold_time.is_none() && seek.is_none() && !aborted_pause && !has_pending_rate {
            return Ok(());
        }
        a.pending = Some(Pending::Play);
        self.update_finished(id, false);
        Ok(())
    }

    /// `pause()` (§4.4.12).
    pub fn pause(&mut self, id: AnimationId) -> Result<(), AnimationError> {
        let current = self.current_time(id);
        let Some(a) = self.animations.get_mut(&id) else {
            return Ok(());
        };
        a.script_play_state = true;
        if a.pending == Some(Pending::Pause) {
            return Ok(());
        }
        if current.is_none() {
            let seek = if a.playback_rate >= 0.0 {
                0.0
            } else {
                let end = a.effect_end();
                if end.is_infinite() {
                    return Err(AnimationError::InvalidState);
                }
                end
            };
            a.hold_time = Some(seek);
        }
        a.pending = Some(Pending::Pause);
        self.update_finished(id, false);
        Ok(())
    }

    /// `finish()` (§4.4.13).
    pub fn finish(&mut self, id: AnimationId) -> Result<(), AnimationError> {
        let Some(a) = self.animations.get(&id) else {
            return Ok(());
        };
        let rate = a.effective_rate();
        let end = a.effect_end();
        if rate == 0.0 || (rate > 0.0 && end.is_infinite()) {
            return Err(AnimationError::InvalidState);
        }
        self.apply_pending_rate(id);
        let limit = if rate > 0.0 { end } else { 0.0 };
        self.silently_set_current_time(id, Some(limit))?;
        let timeline = self.timeline_time(&self.animations[&id].timeline);
        let a = self.animations.get_mut(&id).expect("present");
        if a.start_time.is_none() {
            if let Some(t) = timeline {
                a.start_time = Some(t - limit / a.playback_rate);
            }
        }
        let ready = match a.pending {
            Some(Pending::Pause) if a.start_time.is_some() => {
                a.hold_time = None;
                true
            }
            Some(Pending::Play) if a.start_time.is_some() => true,
            _ => false,
        };
        if ready {
            a.pending = None;
            self.queue(AnimationEventKind::Ready, id);
        }
        self.update_finished(id, true);
        Ok(())
    }

    /// `cancel()` (§4.4.14), queueing `cancel` and the CSS cancel event.
    pub fn cancel(&mut self, id: AnimationId) {
        if self
            .play_state(id)
            .is_none_or(|s| s == PlayStateValue::Idle)
        {
            return;
        }
        let elapsed = self.css_elapsed(id);
        let a = self.animations.get_mut(&id).expect("present");
        a.pending = None;
        a.hold_time = None;
        a.start_time = None;
        a.finished_notified = false;
        a.last_phase = Phase::Idle;
        let owner = a.owner.clone();
        self.queue(AnimationEventKind::Cancel, id);
        match owner {
            Owner::CssTransition {
                element, property, ..
            } => self.css_event(
                id,
                AnimationEventKind::TransitionCancel,
                element,
                property.name(),
                elapsed,
            ),
            Owner::CssAnimation { element, name, .. } => self.css_event(
                id,
                AnimationEventKind::AnimationCancel,
                element,
                &name,
                elapsed,
            ),
            Owner::Script => {}
        }
    }

    /// `reverse()` (§4.4.17).
    pub fn reverse(&mut self, id: AnimationId) -> Result<(), AnimationError> {
        let Some(a) = self.animations.get(&id) else {
            return Ok(());
        };
        if self.timeline_time(&a.timeline).is_none() {
            return Err(AnimationError::InvalidState);
        }
        let original = a.pending_rate;
        let rate = a.effective_rate();
        self.animations.get_mut(&id).expect("present").pending_rate = Some(-rate);
        if let Err(error) = self.play(id, true) {
            self.animations.get_mut(&id).expect("present").pending_rate = original;
            return Err(error);
        }
        Ok(())
    }

    fn apply_pending_rate(&mut self, id: AnimationId) {
        if let Some(a) = self.animations.get_mut(&id) {
            if let Some(rate) = a.pending_rate.take() {
                a.playback_rate = rate;
            }
        }
    }

    /// `updatePlaybackRate()` (§4.4.16.2).
    pub fn update_playback_rate(
        &mut self,
        id: AnimationId,
        rate: f64,
    ) -> Result<(), AnimationError> {
        let state = self.play_state(id);
        let timeline = match self.animations.get(&id) {
            Some(a) => self.timeline_time(&a.timeline),
            None => return Ok(()),
        };
        let Some(a) = self.animations.get_mut(&id) else {
            return Ok(());
        };
        let previous = a.playback_rate;
        a.pending_rate = Some(rate);
        if a.pending.is_some() {
            return Ok(());
        }
        match state {
            Some(PlayStateValue::Idle | PlayStateValue::Paused) | None => {
                self.apply_pending_rate(id);
            }
            Some(PlayStateValue::Finished) => {
                let unconstrained = match (timeline, a.start_time) {
                    (Some(t), Some(s)) => Some((t - s) * previous),
                    _ => None,
                };
                a.start_time = match (timeline, unconstrained) {
                    (Some(t), Some(u)) if rate != 0.0 => Some(t - u / rate),
                    _ => None,
                };
                a.playback_rate = rate;
                a.pending_rate = None;
                self.update_finished(id, false);
            }
            Some(PlayStateValue::Running) => self.play(id, false)?,
        }
        Ok(())
    }

    /// Set the playback rate directly (the `playbackRate` setter, §4.4.16.1).
    pub fn set_playback_rate(&mut self, id: AnimationId, rate: f64) -> Result<(), AnimationError> {
        let previous = self.current_time(id);
        let Some(a) = self.animations.get_mut(&id) else {
            return Ok(());
        };
        a.pending_rate = None;
        a.playback_rate = rate;
        if previous.is_some() {
            self.set_current_time(id, previous)?;
        }
        Ok(())
    }

    fn silently_set_current_time(
        &mut self,
        id: AnimationId,
        time: Option<f64>,
    ) -> Result<(), AnimationError> {
        let Some(a) = self.animations.get(&id) else {
            return Ok(());
        };
        let Some(seek) = time else {
            return if a.hold_time.is_some() || a.start_time.is_some() {
                Err(AnimationError::InvalidState)
            } else {
                Ok(())
            };
        };
        let timeline = self.timeline_time(&a.timeline);
        let a = self.animations.get_mut(&id).expect("present");
        if a.hold_time.is_some()
            || a.start_time.is_none()
            || timeline.is_none()
            || a.playback_rate == 0.0
        {
            a.hold_time = Some(seek);
        } else if let Some(t) = timeline {
            a.start_time = Some(t - seek / a.playback_rate);
        }
        if timeline.is_none() {
            a.start_time = None;
        }
        a.previous_current_time = None;
        Ok(())
    }

    /// The `currentTime` setter (§4.4.4).
    pub fn set_current_time(
        &mut self,
        id: AnimationId,
        time: Option<f64>,
    ) -> Result<(), AnimationError> {
        self.silently_set_current_time(id, time)?;
        let a = self.animations.get_mut(&id).expect("present");
        a.script_play_state |= time.is_some();
        if a.pending == Some(Pending::Pause) {
            a.hold_time = time;
            if let Some(rate) = a.pending_rate.take() {
                a.playback_rate = rate;
            }
            a.start_time = None;
            a.pending = None;
            self.queue(AnimationEventKind::Ready, id);
        }
        self.update_finished(id, true);
        Ok(())
    }

    /// The `startTime` setter (§4.4.3).
    pub fn set_start_time(&mut self, id: AnimationId, time: Option<f64>) {
        let previous = self.current_time(id);
        let Some(a) = self.animations.get(&id) else {
            return;
        };
        let timeline = self.timeline_time(&a.timeline);
        let a = self.animations.get_mut(&id).expect("present");
        if timeline.is_none() && time.is_some() {
            a.hold_time = None;
        }
        if let Some(rate) = a.pending_rate.take() {
            a.playback_rate = rate;
        }
        a.start_time = time;
        if time.is_some() {
            if a.playback_rate != 0.0 {
                a.hold_time = None;
            }
        } else {
            a.hold_time = previous;
        }
        if a.pending.take().is_some() {
            self.queue(AnimationEventKind::Ready, id);
        }
        self.update_finished(id, true);
    }

    /// `persist()` (§4.4.18).
    pub fn persist(&mut self, id: AnimationId) {
        if let Some(a) = self.animations.get_mut(&id) {
            a.replace_state = ReplaceState::Persisted;
        }
    }

    /// Replace an animation's effect (the `effect` setter) or its timing
    /// and keyframes after script changes them; the finished state is
    /// updated.
    pub fn set_effect(&mut self, id: AnimationId, effect: Option<KeyframeEffect>) {
        if let Some(a) = self.animations.get_mut(&id) {
            a.effect = effect;
            a.script_play_state = true;
        }
        self.update_finished(id, false);
    }

    /// Mutable access to an animation's effect (for `updateTiming()` and
    /// `setKeyframes()`); call [`Self::effect_changed`] afterwards.
    pub fn effect_mut(&mut self, id: AnimationId) -> Option<&mut KeyframeEffect> {
        self.animations.get_mut(&id)?.effect.as_mut()
    }

    /// Update the finished state after an effect changed.
    pub fn effect_changed(&mut self, id: AnimationId) {
        self.update_finished(id, false);
    }

    /// Set the timeline (`timeline` setter, §4.4.1 simplified: the
    /// current time is preserved by holding it).
    pub fn set_timeline(&mut self, id: AnimationId, timeline: TimelineRef) {
        let current = self.current_time(id);
        if let Some(a) = self.animations.get_mut(&id) {
            let progress = matches!(timeline, TimelineRef::Scroll(_));
            a.timeline = timeline;
            if progress && a.start_time.is_some() {
                // A playing animation auto-aligns to a progress-based
                // timeline's range at once.
                a.start_time = Some(if a.playback_rate < 0.0 { 100.0 } else { 0.0 });
                a.hold_time = None;
            } else if a.start_time.is_some() {
                a.hold_time = current;
                a.start_time = None;
                a.pending = Some(Pending::Play);
            }
        }
        self.update_finished(id, false);
    }

    /// Drop an animation that script can no longer reach and that has no
    /// effect (the host's garbage collection hook).
    pub fn forget(&mut self, id: AnimationId) {
        if let Some(a) = self.animations.get(&id) {
            if a.owner == Owner::Script && self.play_state(id) == Some(PlayStateValue::Idle) {
                self.animations.remove(&id);
            }
        }
    }

    /// Whether animation `id` is relevant (Web Animations §5.2: current
    /// or in effect), so `getAnimations()` lists it.
    pub fn is_relevant(&self, id: AnimationId) -> bool {
        let Some(a) = self.animations.get(&id) else {
            return false;
        };
        if a.replace_state == ReplaceState::Removed {
            return false;
        }
        let Some(timing) = self.computed_timing(id) else {
            return false;
        };
        let rate = a.playback_rate;
        let current = timing.phase == Phase::Active
            || (timing.phase == Phase::Before && rate >= 0.0)
            || (timing.phase == Phase::After && rate < 0.0)
            || (a.pending.is_some() && timing.phase != Phase::Idle);
        current || timing.active_time.is_some()
    }

    /// The relevant animations targeting `node` (or, with `subtree`, any
    /// of `nodes`), in composite order: CSS transitions, then CSS
    /// animations, then script animations.
    pub fn animations_of(&self, nodes: &HashSet<NodeId>) -> Vec<AnimationId> {
        let mut transitions = Vec::new();
        let mut css = Vec::new();
        let mut script = Vec::new();
        for a in self.animations.values() {
            let target = match &a.owner {
                Owner::CssTransition { element, .. } | Owner::CssAnimation { element, .. } => {
                    Some(*element)
                }
                Owner::Script => a.effect.as_ref().and_then(|e| e.target),
            };
            if !target.is_some_and(|t| nodes.contains(&t)) || !self.is_relevant(a.id) {
                continue;
            }
            match &a.owner {
                Owner::CssTransition { generation, .. } => transitions.push((*generation, a.id)),
                Owner::CssAnimation { element, index, .. } => css.push(((*element, *index), a.id)),
                Owner::Script => script.push(a.id),
            }
        }
        transitions.sort();
        css.sort();
        transitions
            .into_iter()
            .map(|(_, id)| id)
            .chain(css.into_iter().map(|(_, id)| id))
            .chain(script)
            .collect()
    }

    // ----- Ticking (the "update animations and send events" step) -----

    /// Advance the document timeline to `now` (milliseconds since the time
    /// origin): run pending tasks, update finished states, retire finished
    /// transitions, remove replaced animations and queue CSS events.
    pub fn tick(&mut self, now: f64) {
        self.now = Some(now);
        let ids: Vec<AnimationId> = self.animations.keys().copied().collect();
        for &id in &ids {
            self.run_pending(id);
            self.update_finished(id, false);
        }
        for &id in &ids {
            self.css_events(id);
        }
        // Completed transitions leave the running set.
        for &id in &ids {
            let Some(a) = self.animations.get(&id) else {
                continue;
            };
            if let (
                Owner::CssTransition {
                    element, property, ..
                },
                Some(data),
            ) = (&a.owner, &a.transition)
            {
                if self.play_state(id) == Some(PlayStateValue::Finished) {
                    let (element, property, to) = (*element, *property, data.to.clone());
                    if let Some(state) = self.elements.get_mut(&element) {
                        if state.transitions.get(&property) == Some(&id) {
                            state.transitions.remove(&property);
                            state.completed.insert(property, to);
                        }
                    }
                    self.animations.remove(&id);
                }
            }
        }
        self.remove_replaced();
    }

    fn run_pending(&mut self, id: AnimationId) {
        let Some(a) = self.animations.get(&id) else {
            return;
        };
        let Some(ready) = self.timeline_time(&a.timeline) else {
            return;
        };
        let a = self.animations.get_mut(&id).expect("present");
        match a.pending {
            Some(Pending::Play) if matches!(a.timeline, TimelineRef::Scroll(_)) => {
                // Progress-based timelines auto-align the start time to the
                // range start (or end, playing backwards).
                if let Some(rate) = a.pending_rate.take() {
                    a.playback_rate = rate;
                }
                a.start_time = Some(if a.playback_rate < 0.0 { 100.0 } else { 0.0 });
                a.hold_time = None;
                a.pending = None;
                self.queue(AnimationEventKind::Ready, id);
            }
            Some(Pending::Play) => {
                if let Some(hold) = a.hold_time {
                    if let Some(rate) = a.pending_rate.take() {
                        a.playback_rate = rate;
                    }
                    a.start_time = Some(if a.playback_rate == 0.0 {
                        ready
                    } else {
                        ready - hold / a.playback_rate
                    });
                    if a.playback_rate != 0.0 {
                        a.hold_time = None;
                    }
                } else if let (Some(start), Some(rate)) = (a.start_time, a.pending_rate.take()) {
                    let current = (ready - start) * a.playback_rate;
                    a.playback_rate = rate;
                    a.start_time = Some(if rate == 0.0 {
                        ready
                    } else {
                        ready - current / rate
                    });
                }
                a.pending = None;
                self.queue(AnimationEventKind::Ready, id);
            }
            Some(Pending::Pause) => {
                if let (Some(start), None) = (a.start_time, a.hold_time) {
                    a.hold_time = Some((ready - start) * a.playback_rate);
                }
                if let Some(rate) = a.pending_rate.take() {
                    a.playback_rate = rate;
                }
                a.start_time = None;
                a.pending = None;
                self.queue(AnimationEventKind::Ready, id);
            }
            None => {}
        }
    }

    /// `elapsedTime` for a CSS event now (seconds): the active time.
    fn css_elapsed(&self, id: AnimationId) -> f64 {
        let Some(timing) = self.computed_timing(id) else {
            return 0.0;
        };
        let Some(a) = self.animations.get(&id) else {
            return 0.0;
        };
        let effect = a.effect.as_ref().map(|e| &e.timing);
        let active = timing.active_time.unwrap_or_else(|| {
            let local = timing.local_time.unwrap_or(0.0);
            let delay = effect.map_or(0.0, |t| t.delay);
            (local - delay).clamp(0.0, timing.active_duration)
        });
        active / 1000.0
    }

    fn css_event(
        &mut self,
        id: AnimationId,
        kind: AnimationEventKind,
        target: NodeId,
        name: &str,
        elapsed: f64,
    ) {
        self.events.push(AnimationEvent {
            kind,
            animation: id,
            target: Some(target),
            name: name.to_string(),
            elapsed_time: elapsed,
            current_time: None,
            timeline_time: None,
        });
    }

    /// CSS Transitions 2 §6 and CSS Animations 2 §4.2: events from the
    /// phase change since the last tick.
    fn css_events(&mut self, id: AnimationId) {
        let Some(a) = self.animations.get(&id) else {
            return;
        };
        let (element, name, transition) = match &a.owner {
            Owner::CssTransition {
                element, property, ..
            } => (*element, property.name().to_string(), true),
            Owner::CssAnimation { element, name, .. } => (*element, name.to_string(), false),
            Owner::Script => return,
        };
        let Some(effect) = &a.effect else {
            return;
        };
        let timing = effect.timing.clone();
        // Events see the phase with fill `both`: a delayed effect is in
        // its before phase, not idle.
        let filled = Timing {
            fill: Fill::Both,
            ..timing.clone()
        };
        let computed = filled.compute(self.current_time_of(a), a.playback_rate < 0.0);
        let (last_phase, last_iteration) = (a.last_phase, a.last_iteration);
        let phase = computed.phase;
        let iteration = computed.current_iteration;
        let active = timing.active_duration();
        let interval_start = (-timing.delay).min(active).max(0.0) / 1000.0;
        let interval_end = (timing.end_time() - timing.delay).min(active).max(0.0) / 1000.0;
        let a = self.animations.get_mut(&id).expect("present");
        a.last_phase = phase;
        a.last_iteration = iteration;
        use AnimationEventKind as K;
        use Phase as P;
        if transition {
            let mut kinds: Vec<(K, f64)> = Vec::new();
            if last_phase == P::Idle && phase != P::Idle {
                kinds.push((K::TransitionRun, interval_start));
            }
            match (last_phase, phase) {
                (P::Idle | P::Before, P::Active) => {
                    kinds.push((K::TransitionStart, interval_start))
                }
                (P::Idle | P::Before, P::After) => {
                    kinds.push((K::TransitionStart, interval_start));
                    kinds.push((K::TransitionEnd, interval_end));
                }
                (P::Active, P::After) => kinds.push((K::TransitionEnd, interval_end)),
                (P::Active, P::Before) => kinds.push((K::TransitionEnd, interval_start)),
                (P::After, P::Active) => kinds.push((K::TransitionStart, interval_end)),
                (P::After, P::Before) => {
                    kinds.push((K::TransitionStart, interval_end));
                    kinds.push((K::TransitionEnd, interval_start));
                }
                _ => {}
            }
            for (kind, elapsed) in kinds {
                self.css_event(id, kind, element, &name, elapsed);
            }
        } else {
            let mut kinds: Vec<(K, f64)> = Vec::new();
            match (last_phase, phase) {
                (P::Idle | P::Before, P::Active) => kinds.push((K::AnimationStart, interval_start)),
                (P::Idle | P::Before, P::After) => {
                    kinds.push((K::AnimationStart, interval_start));
                    kinds.push((K::AnimationEnd, interval_end));
                }
                (P::Active, P::Active) => {
                    if let (Some(prev), Some(now)) = (last_iteration, iteration) {
                        if prev != now && now.is_finite() {
                            let elapsed = ((now - timing.iteration_start).max(0.0)
                                * timing.duration)
                                / 1000.0;
                            kinds.push((K::AnimationIteration, elapsed));
                        }
                    }
                }
                (P::Active, P::After) => kinds.push((K::AnimationEnd, interval_end)),
                (P::Active, P::Before) => kinds.push((K::AnimationEnd, interval_start)),
                (P::After, P::Active) => kinds.push((K::AnimationStart, interval_end)),
                (P::After, P::Before) => {
                    kinds.push((K::AnimationStart, interval_end));
                    kinds.push((K::AnimationEnd, interval_start));
                }
                _ => {}
            }
            for (kind, elapsed) in kinds {
                self.css_event(id, kind, element, &name, elapsed);
            }
        }
    }

    /// Remove replaced animations (Web Animations §5.5): a finished,
    /// filling script animation whose every property is overridden by a
    /// later script animation of the same target.
    fn remove_replaced(&mut self) {
        let mut covered: HashMap<NodeId, HashSet<PropertyId>> = HashMap::new();
        let mut removed = Vec::new();
        let ids: Vec<AnimationId> = self.animations.keys().rev().copied().collect();
        for id in ids {
            let a = &self.animations[&id];
            if a.owner != Owner::Script {
                continue;
            }
            let Some(effect) = &a.effect else {
                continue;
            };
            let Some(target) = effect.target else {
                continue;
            };
            let properties = effect.properties();
            let state = self.play_state(id);
            let filling = matches!(state, Some(PlayStateValue::Finished))
                && self
                    .computed_timing(id)
                    .is_some_and(|t| t.active_time.is_some());
            let set = covered.entry(target).or_default();
            if a.replace_state == ReplaceState::Active
                && filling
                && a.timeline == TimelineRef::Document
                && !properties.is_empty()
                && properties.iter().all(|p| set.contains(p))
            {
                removed.push(id);
            }
            if filling || state == Some(PlayStateValue::Running) {
                set.extend(properties);
            }
        }
        for id in removed.into_iter().rev() {
            if let Some(a) = self.animations.get_mut(&id) {
                a.replace_state = ReplaceState::Removed;
            }
            self.queue(AnimationEventKind::Remove, id);
        }
    }

    // ----- Styling -----

    /// Start a restyle of the whole document: elements not styled before
    /// [`Self::end_restyle`] lose their CSS animations and transitions.
    pub fn begin_restyle(&mut self) {
        self.seen.clear();
        self.used_timelines.clear();
        self.declared_timelines.clear();
    }

    /// Finish a whole-document restyle (see [`Self::begin_restyle`]).
    pub fn end_restyle(&mut self) {
        // Timelines no animation uses any more are dropped.
        let used: HashSet<u64> = self
            .animations
            .values()
            .filter_map(|a| match &a.timeline {
                TimelineRef::Scroll(r) => Some(r.id),
                _ => None,
            })
            .chain(self.used_timelines.iter().copied())
            .collect();
        self.timeline_sources.retain(|id, _| used.contains(id));
        self.scroll_timelines.retain(|id, _| used.contains(id));
        let gone: Vec<NodeId> = self
            .elements
            .keys()
            .filter(|n| !self.seen.contains(n))
            .copied()
            .collect();
        for node in gone {
            if let Some(state) = self.elements.remove(&node) {
                for id in state.transitions.values().chain(&state.css_animations) {
                    self.animations.remove(id);
                }
            }
        }
    }

    /// Style one element: update its CSS animations from `base` (its
    /// style without animation values), start or cancel transitions, and
    /// sample every effect. `compute` restyles the element with extra
    /// animation-origin and transition-origin declarations. `keyframes`
    /// finds an `@keyframes` rule for the element; `rendered` is false for
    /// an element in a `display: none` subtree, which has no CSS
    /// animations or transitions.
    pub fn style_element(
        &mut self,
        node: NodeId,
        base: &ComputedStyle,
        rendered: bool,
        keyframes: &dyn Fn(&str) -> Option<Arc<KeyframesRule>>,
        compute: &mut dyn FnMut(&[PropertyDeclaration], &[PropertyDeclaration]) -> ComputedStyle,
    ) -> Option<AnimatedValues> {
        self.seen.insert(node);
        let now = self.now;
        if rendered {
            self.declare_timelines(node, base);
        }
        if !rendered {
            if let Some(mut state) = self.elements.remove(&node) {
                for id in state.css_animations.drain(..) {
                    self.cancel(id);
                    self.animations.remove(&id);
                }
                for (_, id) in state.transitions.drain() {
                    self.cancel(id);
                    self.animations.remove(&id);
                }
            }
        } else if now.is_some() {
            self.update_css_animations(node, base, keyframes);
        }
        let animations = self.sample_animations(node, base, compute);
        let after_change = if animations.is_empty() {
            None
        } else {
            Some(compute(&animations, &[]))
        };
        let after = after_change.as_ref().unwrap_or(base);
        if rendered && now.is_some() {
            self.update_transitions(node, after);
        }
        let transitions = self.sample_transitions(node);
        if rendered {
            let state = self.elements.entry(node).or_default();
            state.before_change = Some(after.clone());
        }
        if animations.is_empty() && transitions.is_empty() {
            return None;
        }
        Some(AnimatedValues {
            animations,
            transitions,
        })
    }

    fn update_css_animations(
        &mut self,
        node: NodeId,
        base: &ComputedStyle,
        keyframes: &dyn Fn(&str) -> Option<Arc<KeyframesRule>>,
    ) {
        let list = |p| match base.get(p) {
            PropertyValue::Animation(v) => v.clone(),
            _ => AnimationValue::Times(Vec::new()),
        };
        let AnimationValue::Names(names) = list(PropertyId::AnimationName) else {
            return;
        };
        let existing = self
            .elements
            .get(&node)
            .map(|s| s.css_animations.clone())
            .unwrap_or_default();
        let has_any = names.iter().any(|n| n.name().is_some());
        if existing.is_empty() && !has_any {
            return;
        }
        let durations = list(PropertyId::AnimationDuration);
        let delays = list(PropertyId::AnimationDelay);
        let counts = list(PropertyId::AnimationIterationCount);
        let directions = list(PropertyId::AnimationDirection);
        let fills = list(PropertyId::AnimationFillMode);
        let states = list(PropertyId::AnimationPlayState);
        let compositions = list(PropertyId::AnimationComposition);
        let timelines = list(PropertyId::AnimationTimeline);
        let range_starts = list(PropertyId::AnimationRangeStart);
        let range_ends = list(PropertyId::AnimationRangeEnd);
        let easings = match base.get(PropertyId::AnimationTimingFunction) {
            PropertyValue::Easing(list) => list.as_slice().to_vec(),
            _ => vec![EasingFunction::EASE],
        };
        // Match existing animations by name, from the end of the list.
        let mut old: Vec<(Arc<str>, AnimationId)> = existing
            .iter()
            .filter_map(|id| match &self.animations.get(id)?.owner {
                Owner::CssAnimation { name, .. } => Some((name.clone(), *id)),
                _ => None,
            })
            .collect();
        let mut kept = Vec::new();
        for (index, name) in names.iter().enumerate().rev() {
            let Some(name) = name.name() else {
                continue;
            };
            let Some(rule) = keyframes(name) else {
                continue;
            };
            let range = |list: &AnimationValue| match list {
                AnimationValue::Ranges(v) => cycle(v, index).cloned().unwrap_or(RangeEdge::NORMAL),
                _ => RangeEdge::NORMAL,
            };
            let timeline_value = match &timelines {
                AnimationValue::Timelines(v) => cycle(v, index),
                _ => None,
            };
            let timeline = self.css_timeline(
                node,
                timeline_value,
                range(&range_starts),
                range(&range_ends),
            );
            let progress_based = matches!(timeline, TimelineRef::Scroll(_));
            let iterations = match &counts {
                AnimationValue::IterationCounts(v) => cycle(v, index)
                    .copied()
                    .unwrap_or(IterationCount::Number(1.0))
                    .value(),
                _ => 1.0,
            };
            // A progress-based timeline spreads the iterations over the
            // attachment range and ignores time-based delays (Scroll-driven
            // Animations §4.3, CSS Animations 2 `auto` duration).
            let duration = match durations_at(&durations, index) {
                _ if progress_based => {
                    if iterations.is_finite() && iterations > 0.0 {
                        100.0 / iterations
                    } else {
                        0.0
                    }
                }
                AnimationDuration::Time(t) => t.as_millis(),
                AnimationDuration::Auto => 0.0,
            };
            let timing = Timing {
                delay: match &delays {
                    _ if progress_based => 0.0,
                    AnimationValue::Times(v) => cycle(v, index).map_or(0.0, |t| t.as_millis()),
                    _ => 0.0,
                },
                end_delay: 0.0,
                fill: match &fills {
                    AnimationValue::FillModes(v) => {
                        cycle(v, index).copied().unwrap_or(FillMode::None).into()
                    }
                    _ => Fill::None,
                },
                iteration_start: 0.0,
                iterations,
                duration,
                direction: match &directions {
                    AnimationValue::Directions(v) => cycle(v, index)
                        .copied()
                        .unwrap_or(AnimationDirection::Normal),
                    _ => AnimationDirection::Normal,
                },
                easing: EasingFunction::LINEAR,
            };
            let default_easing = cycle(&easings, index)
                .cloned()
                .unwrap_or(EasingFunction::EASE);
            let composite = match &compositions {
                AnimationValue::Compositions(v) => cycle(v, index)
                    .copied()
                    .unwrap_or(CompositeOperation::Replace),
                _ => CompositeOperation::Replace,
            };
            let paused = matches!(&states, AnimationValue::PlayStates(v)
                if cycle(v, index) == Some(&PlayState::Paused));
            let mut effect = css_keyframe_effect(node, &rule, timing, default_easing);
            effect.composite = composite;
            let found = old.iter().rposition(|(n, _)| &**n == name);
            let id = match found {
                Some(position) => {
                    let (_, id) = old.remove(position);
                    let a = self.animations.get_mut(&id).expect("present");
                    if let Owner::CssAnimation { index: i, .. } = &mut a.owner {
                        *i = index;
                    }
                    let timing_changed =
                        a.effect.as_ref().map(|e| &e.timing) != Some(&effect.timing);
                    a.effect = Some(effect);
                    let timeline_changed = a.timeline != timeline;
                    if timeline_changed {
                        self.set_timeline(id, timeline.clone());
                    } else if timing_changed {
                        self.update_finished(id, false);
                    }
                    id
                }
                None => {
                    let id = self.create_owned(
                        Owner::CssAnimation {
                            element: node,
                            name: name.into(),
                            index,
                        },
                        Some(effect),
                        timeline.clone(),
                    );
                    let a = self.animations.get_mut(&id).expect("present");
                    if paused {
                        a.hold_time = Some(0.0);
                        a.css_paused = true;
                    } else if progress_based {
                        // Auto-aligned start time: the start of the range.
                        a.start_time = Some(0.0);
                    } else {
                        a.start_time = self.now;
                    }
                    id
                }
            };
            let a = self.animations.get_mut(&id).expect("present");
            if !a.script_play_state && a.css_paused != paused {
                a.css_paused = paused;
                if paused {
                    let _ = self.pause(id);
                    self.run_pending(id);
                } else {
                    let _ = self.play(id, false);
                    self.run_pending(id);
                }
                if let Some(a) = self.animations.get_mut(&id) {
                    a.script_play_state = false;
                }
            }
            kept.push(id);
        }
        kept.reverse();
        for (_, id) in old {
            self.cancel(id);
            self.animations.remove(&id);
        }
        self.elements.entry(node).or_default().css_animations = kept;
    }

    fn sample_animations(
        &mut self,
        node: NodeId,
        base: &ComputedStyle,
        compute: &mut dyn FnMut(&[PropertyDeclaration], &[PropertyDeclaration]) -> ComputedStyle,
    ) -> Vec<PropertyDeclaration> {
        let css = self
            .elements
            .get(&node)
            .map(|s| s.css_animations.clone())
            .unwrap_or_default();
        let script: Vec<AnimationId> = self
            .animations
            .values()
            .filter(|a| {
                a.owner == Owner::Script
                    && a.replace_state != ReplaceState::Removed
                    && a.effect.as_ref().is_some_and(|e| e.target == Some(node))
            })
            .map(|a| a.id)
            .collect();
        if css.is_empty() && script.is_empty() {
            return Vec::new();
        }
        // The effect stack, starting from the underlying (base) values.
        let mut stack: BTreeMap<PropertyId, PropertyValue> = BTreeMap::new();
        for id in css.into_iter().chain(script) {
            let Some(timing) = self.computed_timing(id) else {
                continue;
            };
            let (Some(progress), Some(iteration)) = (timing.progress, timing.current_iteration)
            else {
                continue;
            };
            let a = &self.animations[&id];
            let Some(effect) = &a.effect else {
                continue;
            };
            let values = sample_effect(effect, progress, iteration, base, &stack, compute);
            stack.extend(values);
        }
        stack.into_iter().map(|(p, v)| declaration(p, v)).collect()
    }

    fn update_transitions(&mut self, node: NodeId, after: &ComputedStyle) {
        let Some(before) = self
            .elements
            .get(&node)
            .and_then(|s| s.before_change.clone())
        else {
            return;
        };
        let now = self.now.unwrap_or(0.0);
        let list = |p| match after.get(p) {
            PropertyValue::Animation(v) => v.clone(),
            _ => AnimationValue::Times(Vec::new()),
        };
        let AnimationValue::TransitionProperties(names) = list(PropertyId::TransitionProperty)
        else {
            return;
        };
        let durations = list(PropertyId::TransitionDuration);
        let delays = list(PropertyId::TransitionDelay);
        let behaviors = list(PropertyId::TransitionBehavior);
        let easings = match after.get(PropertyId::TransitionTimingFunction) {
            PropertyValue::Easing(list) => list.as_slice().to_vec(),
            _ => vec![EasingFunction::EASE],
        };
        // The matching transition-property entry of each longhand: the
        // last one naming it.
        let mut matching: HashMap<PropertyId, usize> = HashMap::new();
        for (index, entry) in names.iter().enumerate() {
            for &p in entry.longhands() {
                matching.insert(p, index);
            }
        }
        let _ = TransitionProperty::All;
        let state_has = |s: &Self| {
            s.elements
                .get(&node)
                .is_some_and(|e| !e.transitions.is_empty() || !e.completed.is_empty())
        };
        if matching.is_empty() && !state_has(self) {
            return;
        }
        for &property in PropertyId::ALL {
            if animation_type(property) == AnimationType::NotAnimatable {
                continue;
            }
            let (from, to) = (before.get(property), after.get(property));
            let entry = matching.get(&property).copied();
            let times = |list: &AnimationValue, index: usize| match list {
                AnimationValue::Times(v) => cycle(v, index).map_or(0.0, |t| t.as_millis()),
                _ => 0.0,
            };
            let (duration, delay) =
                entry.map_or((0.0, 0.0), |i| (times(&durations, i), times(&delays, i)));
            let allow_discrete = entry.is_some_and(|i| {
                matches!(&behaviors, AnimationValue::TransitionBehaviors(v)
                    if cycle(v, i) == Some(&TransitionBehavior::AllowDiscrete))
            });
            let transitionable = |a: &PropertyValue, b: &PropertyValue| {
                animation_type(property) != AnimationType::NotAnimatable
                    && (interpolable(property, a, b) || allow_discrete)
            };
            let combined = duration.max(0.0) + delay;
            let easing = entry
                .and_then(|i| cycle(&easings, i).cloned())
                .unwrap_or(EasingFunction::EASE);
            let running = self
                .elements
                .get(&node)
                .and_then(|s| s.transitions.get(&property).copied());
            let completed = self
                .elements
                .get(&node)
                .and_then(|s| s.completed.get(&property).cloned());
            // Step 1: start a transition.
            if running.is_none()
                && from != to
                && entry.is_some()
                && combined > 0.0
                && transitionable(from, to)
                && completed.as_ref() != Some(to)
            {
                if completed.is_some() {
                    self.elements
                        .entry(node)
                        .or_default()
                        .completed
                        .remove(&property);
                }
                self.start_transition(
                    node,
                    property,
                    from.clone(),
                    to.clone(),
                    from.clone(),
                    1.0,
                    duration,
                    delay,
                    easing.clone(),
                    now,
                );
                continue;
            }
            // Step 2: a completed transition with another end value.
            if completed.as_ref().is_some_and(|c| c != to) {
                self.elements
                    .entry(node)
                    .or_default()
                    .completed
                    .remove(&property);
            }
            // Step 3: no transition for this property any more.
            if entry.is_none() || combined <= 0.0 {
                if let Some(id) = running {
                    self.cancel(id);
                    self.animations.remove(&id);
                    self.elements
                        .entry(node)
                        .or_default()
                        .transitions
                        .remove(&property);
                }
                self.elements
                    .entry(node)
                    .or_default()
                    .completed
                    .remove(&property);
                continue;
            }
            // Step 4: a running transition with another end value.
            let Some(id) = running else {
                continue;
            };
            let Some(data) = self.animations.get(&id).and_then(|a| a.transition.clone()) else {
                continue;
            };
            if data.to == *to {
                continue;
            }
            let current = self
                .transition_value(id)
                .unwrap_or_else(|| data.from.clone());
            if current == *to || !transitionable(&current, to) {
                self.cancel(id);
                self.animations.remove(&id);
                self.elements
                    .entry(node)
                    .or_default()
                    .transitions
                    .remove(&property);
                continue;
            }
            self.cancel(id);
            self.animations.remove(&id);
            self.elements
                .entry(node)
                .or_default()
                .transitions
                .remove(&property);
            if data.adjusted_start == *to {
                // Reversing: shorten by how far the old one got.
                let progress = self.computed_timing_of_removed(&data, id).unwrap_or(0.0);
                let factor = (progress * data.shortening + (1.0 - data.shortening))
                    .abs()
                    .clamp(0.0, 1.0);
                let delay = if delay < 0.0 { delay * factor } else { delay };
                self.start_transition(
                    node,
                    property,
                    current,
                    to.clone(),
                    data.to.clone(),
                    factor,
                    duration * factor,
                    delay,
                    easing,
                    now,
                );
            } else {
                self.start_transition(
                    node,
                    property,
                    current,
                    to.clone(),
                    from.clone(),
                    1.0,
                    duration,
                    delay,
                    easing,
                    now,
                );
            }
        }
    }

    /// The eased output progress a just-cancelled transition had reached
    /// (its effect's transformed progress before cancelling).
    fn computed_timing_of_removed(&self, _data: &TransitionData, _id: AnimationId) -> Option<f64> {
        self.last_transition_progress
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the transition's defining values"
    )]
    fn start_transition(
        &mut self,
        node: NodeId,
        property: PropertyId,
        from: PropertyValue,
        to: PropertyValue,
        adjusted_start: PropertyValue,
        shortening: f64,
        duration: f64,
        delay: f64,
        easing: EasingFunction,
        now: f64,
    ) {
        self.transition_generation += 1;
        let keyframes = vec![
            EffectKeyframe {
                offset: Some(0.0),
                computed_offset: 0.0,
                easing,
                composite: None,
                values: KeyframeValues::Computed(vec![(property, from.clone())]),
            },
            EffectKeyframe {
                offset: Some(1.0),
                computed_offset: 1.0,
                easing: EasingFunction::LINEAR,
                composite: None,
                values: KeyframeValues::Computed(vec![(property, to.clone())]),
            },
        ];
        let timing = Timing {
            delay,
            duration: duration.max(0.0),
            fill: Fill::Backwards,
            ..Timing::default()
        };
        let id = self.create_owned(
            Owner::CssTransition {
                element: node,
                property,
                generation: self.transition_generation,
            },
            Some(KeyframeEffect::new(Some(node), keyframes, timing)),
            TimelineRef::Document,
        );
        let a = self.animations.get_mut(&id).expect("present");
        a.start_time = Some(now);
        a.transition = Some(TransitionData {
            property,
            from,
            to,
            adjusted_start,
            shortening,
        });
        let state = self.elements.entry(node).or_default();
        state.transitions.insert(property, id);
        state.completed.remove(&property);
    }

    /// A running transition's current value (and remember its progress
    /// for reversing).
    fn transition_value(&mut self, id: AnimationId) -> Option<PropertyValue> {
        let timing = self.computed_timing(id)?;
        let a = self.animations.get(&id)?;
        let data = a.transition.as_ref()?;
        let progress = timing.progress?;
        let first = a.effect.as_ref()?.keyframes.first()?;
        let eased = first.easing.sample(progress, false);
        self.last_transition_progress = Some(timing.simple_progress.unwrap_or(progress));
        Some(interpolate(data.property, &data.from, &data.to, eased))
    }

    fn sample_transitions(&mut self, node: NodeId) -> Vec<PropertyDeclaration> {
        let Some(state) = self.elements.get(&node) else {
            return Vec::new();
        };
        let mut running: Vec<(u64, PropertyId, AnimationId)> = state
            .transitions
            .iter()
            .filter_map(|(p, id)| match &self.animations.get(id)?.owner {
                Owner::CssTransition { generation, .. } => Some((*generation, *p, *id)),
                _ => None,
            })
            .collect();
        running.sort();
        running
            .into_iter()
            .filter_map(|(_, p, id)| Some(declaration(p, self.transition_value(id)?)))
            .collect()
    }
}

fn durations_at(list: &AnimationValue, index: usize) -> AnimationDuration {
    match list {
        AnimationValue::Durations(v) => cycle(v, index).copied().unwrap_or(AnimationDuration::Auto),
        _ => AnimationDuration::Auto,
    }
}

/// The keyframe effect of a CSS animation (CSS Animations 2 §3): keyframes
/// at one offset merge (later declarations win), `animation-timing-function`
/// in a keyframe is its easing and `animation-composition` its composite.
fn css_keyframe_effect(
    node: NodeId,
    rule: &KeyframesRule,
    timing: Timing,
    default_easing: EasingFunction,
) -> KeyframeEffect {
    // (offset, easing, composite) → declarations, merged in source order.
    let mut frames: Vec<RawFrame> = Vec::new();
    for keyframe in &rule.keyframes {
        let mut easing = None;
        let mut composite = None;
        let mut declarations = Vec::new();
        for d in &keyframe.declarations {
            match (d.property.longhand(), &d.value) {
                (
                    Some(PropertyId::AnimationTimingFunction),
                    DeclaredValue::Value(PropertyValue::Easing(list)),
                ) => {
                    easing = list.as_slice().first().cloned();
                }
                (
                    Some(PropertyId::AnimationComposition),
                    DeclaredValue::Value(PropertyValue::Animation(AnimationValue::Compositions(v))),
                ) => composite = v.first().copied(),
                (Some(p), _) if animation_type(p) == AnimationType::NotAnimatable => {}
                _ => declarations.push(d.clone()),
            }
        }
        for selector in &keyframe.selectors {
            let KeyframeSelector::Offset(offset) = selector else {
                // Range keyframes need a view timeline (scroll-driven
                // animations); time-based animations ignore them.
                continue;
            };
            match frames
                .iter_mut()
                .find(|(o, e, c, _)| *o == *offset && *e == easing && *c == composite)
            {
                Some((_, _, _, existing)) => {
                    for d in &declarations {
                        existing.retain(|x| x.property != d.property);
                        existing.push(d.clone());
                    }
                }
                None => frames.push((*offset, easing.clone(), composite, declarations.clone())),
            }
        }
    }
    frames.sort_by(|a, b| a.0.total_cmp(&b.0));
    let keyframes = frames
        .into_iter()
        .map(|(offset, easing, composite, declarations)| EffectKeyframe {
            offset: Some(offset),
            computed_offset: offset,
            easing: easing.unwrap_or_else(|| default_easing.clone()),
            composite,
            values: KeyframeValues::Declarations(declarations),
        })
        .collect();
    KeyframeEffect {
        target: Some(node),
        keyframes,
        composite: CompositeOperation::Replace,
        timing,
    }
}

/// The effect value of a keyframe effect for each of its properties (Web
/// Animations §5.3.4), on top of `stack` (the values of earlier effects,
/// or the base values).
fn sample_effect(
    effect: &KeyframeEffect,
    progress: f64,
    _iteration: f64,
    base: &ComputedStyle,
    stack: &BTreeMap<PropertyId, PropertyValue>,
    compute: &mut dyn FnMut(&[PropertyDeclaration], &[PropertyDeclaration]) -> ComputedStyle,
) -> Vec<(PropertyId, PropertyValue)> {
    // Each keyframe's computed values, computed once.
    let mut computed: Vec<Option<ComputedStyle>> = vec![None; effect.keyframes.len()];
    let mut out = Vec::new();
    for property in effect.properties() {
        let underlying = stack
            .get(&property)
            .unwrap_or_else(|| base.get(property))
            .clone();
        // Property-specific keyframes: (offset, value, easing, composite).
        let mut frames: Vec<(f64, PropertyValue, EasingFunction, CompositeOperation)> = Vec::new();
        for (i, k) in effect.keyframes.iter().enumerate() {
            let value = match &k.values {
                KeyframeValues::Computed(values) => values
                    .iter()
                    .find(|(p, _)| *p == property)
                    .map(|(_, v)| v.clone()),
                KeyframeValues::Declarations(declarations) => {
                    if !declarations.iter().any(|d| d.property == property) {
                        None
                    } else {
                        let style = computed[i].get_or_insert_with(|| compute(declarations, &[]));
                        Some(style.get(property).clone())
                    }
                }
            };
            if let Some(value) = value {
                let op = k.composite.unwrap_or(effect.composite);
                frames.push((k.computed_offset, value, k.easing.clone(), op));
            }
        }
        if frames.is_empty() {
            continue;
        }
        // Neutral keyframes at 0 and 1 hold the underlying value.
        if !frames.iter().any(|f| f.0 == 0.0) {
            frames.insert(
                0,
                (
                    0.0,
                    underlying.clone(),
                    frames[0].2.clone(),
                    CompositeOperation::Replace,
                ),
            );
        }
        if !frames.iter().any(|f| f.0 == 1.0) {
            frames.push((
                1.0,
                underlying.clone(),
                EasingFunction::LINEAR,
                CompositeOperation::Replace,
            ));
        }
        let resolve = |f: &(f64, PropertyValue, EasingFunction, CompositeOperation)| {
            composite(property, &underlying, &f.1, f.3)
        };
        let zeros = frames.iter().filter(|f| f.0 == 0.0).count();
        let ones = frames.iter().filter(|f| f.0 == 1.0).count();
        let value = if progress < 0.0 && zeros > 1 {
            resolve(&frames[0])
        } else if progress >= 1.0 && ones > 1 {
            resolve(frames.last().expect("nonempty"))
        } else {
            let start = frames
                .iter()
                .rposition(|f| f.0 <= progress && f.0 < 1.0)
                .unwrap_or(0);
            let end = (start + 1).min(frames.len() - 1);
            let (a, b) = (&frames[start], &frames[end]);
            if start == end || b.0 == a.0 {
                resolve(a)
            } else {
                let local = (progress - a.0) / (b.0 - a.0);
                let eased = a.2.sample(local, false);
                interpolate(property, &resolve(a), &resolve(b), eased)
            }
        };
        out.push((property, value));
    }
    out
}
