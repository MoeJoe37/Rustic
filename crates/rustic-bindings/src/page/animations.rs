//! The Web Animations API ([Web Animations 1] §6): `Animation`,
//! `AnimationEffect`, `KeyframeEffect`, `AnimationTimeline`,
//! `DocumentTimeline`, `CSSAnimation`, `CSSTransition`,
//! `Element.animate()`, `getAnimations()` and `document.timeline`, over
//! the document's [`AnimationController`] that the renderer shares.
//!
//! An Animation wrapper is created on demand and held, with its
//! controller entry, while the animation exists; finished script
//! animations that no longer affect anything are dropped oldest first
//! past [`KEPT_FINISHED`] (TD-203). A detached `KeyframeEffect` keeps its
//! native effect in the page until the page goes away (TD-203).
//!
//! [Web Animations 1]: https://drafts.csswg.org/web-animations-1/

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use rustic_css::{
    AnimationDirection, CompositeOperation, CssParser, EasingFunction, PropertyDeclaration,
    PropertyId, PropertyValue, parse_declaration,
};
use rustic_dom::{NodeId, NodeKind};
use rustic_js::runtime::{
    Abrupt, Agent, HostRoot, JsResult, NativeFn, ObjectId, PropertyDescriptor, PropertyKey, Value,
};
use rustic_style::{
    AnimationController, AnimationError, AnimationEventKind, AnimationId, EffectKeyframe, Fill,
    KeyframeEffect, KeyframeValues, Owner, PlayStateValue, TimelineRef, Timing,
};

use crate::heap::WrapperKind;
use crate::webidl::to_usv_string;

use super::events::{self, PrivateArray, define_method, with_root};
use super::{
    creation, identity, illegal_constructor, install_interface, prototype, required_arg, with_page,
    wrap,
};

const ANIMATION: &str = "Animation";
const CSS_ANIMATION: &str = "CSSAnimation";
const CSS_TRANSITION: &str = "CSSTransition";
const EFFECT: &str = "AnimationEffect";
const KEYFRAME_EFFECT: &str = "KeyframeEffect";
const TIMELINE: &str = "AnimationTimeline";
const DOCUMENT_TIMELINE: &str = "DocumentTimeline";
const SCROLL_TIMELINE: &str = "ScrollTimeline";
const VIEW_TIMELINE: &str = "ViewTimeline";

/// Animation wrapper slots.
const A_ID: u32 = 0;
const A_EFFECT: u32 = 1;
const A_TIMELINE: u32 = 2;
const A_READY: u32 = 3;
const A_FINISHED: u32 = 4;
const A_NAME: u32 = 5;

/// KeyframeEffect slots: `[serial]`.
const E_SERIAL: u32 = 0;

/// Timeline slots: `[origin time in ms, kind, source or subject element,
/// axis]`; the last three only for scroll and view timelines.
const T_ORIGIN: u32 = 0;
const T_KIND: u32 = 1;
const T_ELEMENT: u32 = 2;
const T_AXIS: u32 = 3;

/// Timeline kinds.
const KIND_SCROLL: f64 = 1.0;
const KIND_VIEW: f64 = 2.0;
const AXES: [(&str, rustic_css::ScrollAxis); 4] = [
    ("block", rustic_css::ScrollAxis::Block),
    ("inline", rustic_css::ScrollAxis::Inline),
    ("x", rustic_css::ScrollAxis::X),
    ("y", rustic_css::ScrollAxis::Y),
];

/// Finished, irrelevant script animations kept for replay before the
/// oldest are dropped.
pub(super) const KEPT_FINISHED: usize = 256;

#[derive(Debug)]
enum EffectHome {
    Detached(KeyframeEffect),
    Attached(AnimationId),
}

/// Page-lifetime animation state.
#[derive(Debug)]
pub(super) struct Animations {
    pub(super) controller: Rc<RefCell<AnimationController>>,
    /// Animation wrapper → slots.
    states: ObjectId,
    /// KeyframeEffect object → slots.
    effects: ObjectId,
    /// Timeline object → slots.
    timelines: ObjectId,
    /// The `document.timeline` object (rooted).
    document_timeline: Value,
    /// Wrappers by animation, rooted while the animation exists.
    wrappers: RefCell<HashMap<AnimationId, (Value, HostRoot)>>,
    homes: RefCell<HashMap<u64, EffectHome>>,
    /// The effect serial of each animation's effect.
    effect_of: RefCell<HashMap<AnimationId, u64>>,
    next_serial: Cell<u64>,
    /// Finished script animations in the order they finished.
    finished: RefCell<Vec<AnimationId>>,
}

impl Animations {
    pub(super) fn new(
        agent: &mut Agent,
        controller: Option<Rc<RefCell<AnimationController>>>,
    ) -> JsResult<Self> {
        let states = agent.new_weak_map()?;
        agent.root(Value::object(states))?;
        let effects = agent.new_weak_map()?;
        agent.root(Value::object(effects))?;
        let timelines = agent.new_weak_map()?;
        agent.root(Value::object(timelines))?;
        Ok(Self {
            controller: controller
                .unwrap_or_else(|| Rc::new(RefCell::new(AnimationController::new()))),
            states,
            effects,
            timelines,
            document_timeline: Value::UNDEFINED,
            wrappers: RefCell::default(),
            homes: RefCell::default(),
            effect_of: RefCell::default(),
            next_serial: Cell::new(0),
            finished: RefCell::default(),
        })
    }

    pub(super) fn set_document_timeline(&mut self, timeline: Value) {
        self.document_timeline = timeline;
    }
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        TIMELINE,
        None,
        illegal_constructor,
        0,
        &[("currentTime", "get currentTime", timeline_current_time)],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        DOCUMENT_TIMELINE,
        Some(TIMELINE),
        construct_timeline,
        0,
        &[],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        SCROLL_TIMELINE,
        Some(TIMELINE),
        construct_scroll_timeline::<false>,
        0,
        &[
            ("source", "get source", timeline_element::<false>),
            ("axis", "get axis", timeline_axis),
        ],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        VIEW_TIMELINE,
        Some(SCROLL_TIMELINE),
        construct_scroll_timeline::<true>,
        0,
        &[("subject", "get subject", timeline_element::<true>)],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        EFFECT,
        None,
        illegal_constructor,
        0,
        &[],
        |agent, proto| {
            for (name, method, length) in [
                ("getTiming", get_timing as NativeFn, 0),
                ("getComputedTiming", get_computed_timing, 0),
                ("updateTiming", update_timing, 0),
            ] {
                define_method(agent, proto, name, method, length)?;
            }
            Ok(())
        },
    )?;
    install_interface(
        agent,
        KEYFRAME_EFFECT,
        Some(EFFECT),
        construct_effect,
        1,
        &[("pseudoElement", "get pseudoElement", null_getter)],
        |agent, proto| {
            accessor(agent, proto, "target", effect_target, set_effect_target)?;
            accessor(
                agent,
                proto,
                "composite",
                effect_composite,
                set_effect_composite,
            )?;
            define_method(agent, proto, "getKeyframes", get_keyframes, 0)?;
            define_method(agent, proto, "setKeyframes", set_keyframes, 1)
        },
    )?;
    install_interface(
        agent,
        ANIMATION,
        Some(events::TARGET),
        construct_animation,
        0,
        &[
            ("playState", "get playState", play_state),
            ("replaceState", "get replaceState", replace_state),
            ("pending", "get pending", pending),
            ("ready", "get ready", ready),
            ("finished", "get finished", finished),
        ],
        |agent, proto| {
            accessor(agent, proto, "id", animation_id, set_animation_id)?;
            accessor(
                agent,
                proto,
                "effect",
                animation_effect,
                set_animation_effect,
            )?;
            accessor(
                agent,
                proto,
                "timeline",
                animation_timeline,
                set_animation_timeline,
            )?;
            accessor(agent, proto, "startTime", start_time, set_start_time)?;
            accessor(agent, proto, "currentTime", current_time, set_current_time)?;
            accessor(
                agent,
                proto,
                "playbackRate",
                playback_rate,
                set_playback_rate,
            )?;
            events::install_animation_handlers(agent, proto)?;
            for (name, method, length) in [
                ("cancel", cancel as NativeFn, 0),
                ("finish", finish, 0),
                ("play", play, 0),
                ("pause", pause, 0),
                ("updatePlaybackRate", update_playback_rate, 1),
                ("reverse", reverse, 0),
                ("persist", persist, 0),
                ("commitStyles", commit_styles, 0),
            ] {
                define_method(agent, proto, name, method, length)?;
            }
            Ok(())
        },
    )?;
    install_interface(
        agent,
        CSS_ANIMATION,
        Some(ANIMATION),
        illegal_constructor,
        0,
        &[("animationName", "get animationName", css_name)],
        |_, _| Ok(()),
    )?;
    install_interface(
        agent,
        CSS_TRANSITION,
        Some(ANIMATION),
        illegal_constructor,
        0,
        &[("transitionProperty", "get transitionProperty", css_name)],
        |_, _| Ok(()),
    )?;
    let element = prototype(agent, "Element")?;
    define_method(agent, element, "animate", animate, 1)?;
    define_method(agent, element, "getAnimations", element_get_animations, 0)?;
    let document = prototype(agent, "Document")?;
    define_method(agent, document, "getAnimations", document_get_animations, 0)?;
    let get = agent.new_native_function(document_timeline, "get timeline", 0)?;
    let key = agent.heap.key("timeline")?;
    agent.define_property_or_throw(
        document,
        key,
        PropertyDescriptor::accessor(Value::object(get), Value::UNDEFINED, true, true),
    )
}

/// Create the rooted `document.timeline`; call once the page exists.
pub(super) fn create_document_timeline(agent: &mut Agent) -> JsResult<Value> {
    let proto = prototype(agent, DOCUMENT_TIMELINE)?;
    let object = Value::object(agent.new_object_with(Some(proto))?);
    agent.root(object)?;
    Ok(object)
}

pub(super) fn register_document_timeline(agent: &mut Agent, timeline: Value) -> JsResult<()> {
    with_page(agent, |agent, page| {
        let slots = PrivateArray::new(agent, &[Value::number(0.0)])?;
        agent.weak_map_set(page.animations.timelines, timeline, slots.value())
    })
}

fn accessor(
    agent: &mut Agent,
    proto: ObjectId,
    name: &str,
    getter: NativeFn,
    setter: NativeFn,
) -> JsResult<()> {
    let get = agent.new_native_function(getter, &format!("get {name}"), 0)?;
    let set = agent.new_native_function(setter, &format!("set {name}"), 1)?;
    let key = agent.heap.key(name)?;
    agent.define_property_or_throw(
        proto,
        key,
        PropertyDescriptor::accessor(Value::object(get), Value::object(set), true, true),
    )
}

fn null_getter(_: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::NULL)
}

fn controller(agent: &mut Agent) -> JsResult<Rc<RefCell<AnimationController>>> {
    with_page(agent, |_, page| Ok(Rc::clone(&page.animations.controller)))
}

fn changed(agent: &mut Agent) -> JsResult<()> {
    with_page(agent, |_, page| {
        page.tree.animations_changed();
        Ok(())
    })
}

fn invalid_state(agent: &mut Agent, error: AnimationError) -> Abrupt {
    match error {
        AnimationError::InvalidState => creation::exception(
            agent,
            "InvalidStateError",
            "the animation cannot do that in its current state",
        ),
    }
}

fn time_value(time: Option<f64>) -> Value {
    time.map_or(Value::NULL, Value::number)
}

// ---- Timelines ----------------------------------------------------------

fn construct_timeline(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("DocumentTimeline constructor requires new");
    };
    let mut origin = 0.0;
    if let Some(object) = args.first().and_then(|v| v.as_object()) {
        let key = agent.heap.key("originTime")?;
        let value = agent.get(object, key)?;
        if value != Value::UNDEFINED {
            origin = finite(agent, value, "originTime")?;
        }
    }
    let proto = agent.host_prototype_from_new_target(DOCUMENT_TIMELINE, new_target)?;
    let object = Value::object(agent.new_object_with(Some(proto))?);
    with_root(agent, object, |agent| {
        let slots = PrivateArray::new(agent, &[Value::number(origin)])?;
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.animations.timelines, object, slots.value())
        })?;
        Ok(object)
    })
}

fn timeline_origin(agent: &mut Agent, timeline: Value) -> JsResult<Option<f64>> {
    let slots = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.animations.timelines, timeline))
    })?;
    match PrivateArray::from_value(slots) {
        Some(slots) => Ok(slots.get(agent, T_ORIGIN)?.as_number()),
        None => Ok(None),
    }
}

/// `new ScrollTimeline({source, axis})` and `new ViewTimeline({subject,
/// axis})` (Scroll-driven Animations §2.3, §3.3). `currentTime` is a
/// percentage number rather than a `CSSUnitValue` (TD-206).
fn construct_scroll_timeline<const VIEW: bool>(
    agent: &mut Agent,
    _: Value,
    args: &[Value],
) -> JsResult<Value> {
    let interface = if VIEW { VIEW_TIMELINE } else { SCROLL_TIMELINE };
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("timeline constructor requires new");
    };
    let options = args.first().copied().unwrap_or(Value::UNDEFINED);
    // An absent source is the document's scrolling element (the
    // viewport, kept as undefined); null leaves the timeline inactive.
    let (mut element, mut axis) = (if VIEW { Value::NULL } else { Value::UNDEFINED }, 0.0);
    if let Some(object) = options.as_object() {
        let value = get(agent, object, if VIEW { "subject" } else { "source" })?;
        if value != Value::UNDEFINED {
            if value != Value::NULL {
                element_id(agent, value)?;
            }
            element = value;
        }
        let value = get(agent, object, "axis")?;
        if value != Value::UNDEFINED {
            let text = to_usv_string(agent, value)?;
            let Some(index) = AXES.iter().position(|(name, _)| *name == text) else {
                return agent.type_error("axis must be block, inline, x or y");
            };
            axis = index as f64;
        }
    }
    let proto = agent.host_prototype_from_new_target(interface, new_target)?;
    let object = Value::object(agent.new_object_with(Some(proto))?);
    with_root(agent, object, |agent| {
        let kind = if VIEW { KIND_VIEW } else { KIND_SCROLL };
        let slots = PrivateArray::new(
            agent,
            &[
                Value::number(0.0),
                Value::number(kind),
                element,
                Value::number(axis),
            ],
        )?;
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.animations.timelines, object, slots.value())
        })?;
        Ok(object)
    })
}

fn timeline_slots(agent: &mut Agent, timeline: Value) -> JsResult<Option<PrivateArray>> {
    let slots = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.animations.timelines, timeline))
    })?;
    Ok(PrivateArray::from_value(slots))
}

/// A scroll or view timeline object's source, or `None` for a document
/// timeline; `Some(None)` is an inactive one.
fn scroll_source(
    agent: &mut Agent,
    timeline: Value,
) -> JsResult<Option<Option<rustic_style::TimelineSource>>> {
    let Some(slots) = timeline_slots(agent, timeline)? else {
        return Ok(None);
    };
    let kind = slots.get(agent, T_KIND)?.as_number();
    if kind != Some(KIND_SCROLL) && kind != Some(KIND_VIEW) {
        return Ok(None);
    }
    let element = slots.get(agent, T_ELEMENT)?;
    let axis = slots.get(agent, T_AXIS)?.as_number().unwrap_or(0.0) as usize;
    let axis = AXES
        .get(axis)
        .map_or(rustic_css::ScrollAxis::Block, |(_, a)| *a);
    let node = match element {
        Value::UNDEFINED => None,
        Value::NULL => return Ok(Some(None)),
        element => Some(element_id(agent, element)?),
    };
    Ok(Some(Some(match (kind == Some(KIND_VIEW), node) {
        (true, Some(subject)) => rustic_style::TimelineSource::View {
            subject,
            axis,
            inset: rustic_css::ViewInset(
                rustic_css::LengthPercentageOrAuto::Auto,
                rustic_css::LengthPercentageOrAuto::Auto,
            ),
        },
        (true, None) => return Ok(Some(None)),
        (false, Some(node)) => rustic_style::TimelineSource::Scroll {
            scroller: rustic_style::ScrollerRef::Element(node),
            axis,
        },
        (false, None) => rustic_style::TimelineSource::Scroll {
            scroller: rustic_style::ScrollerRef::Root,
            axis,
        },
    })))
}

fn timeline_element<const VIEW: bool>(
    agent: &mut Agent,
    this: Value,
    _: &[Value],
) -> JsResult<Value> {
    let Some(slots) = timeline_slots(agent, this)? else {
        return agent.type_error("expected a ScrollTimeline receiver");
    };
    let kind = slots.get(agent, T_KIND)?.as_number();
    let element = slots.get(agent, T_ELEMENT)?;
    match (VIEW, kind) {
        (false, Some(KIND_SCROLL)) if element == Value::UNDEFINED => {
            // The viewport's scrolling element.
            let root = with_page(agent, |agent, page| {
                page.read(agent, page.main, |_, document| {
                    Ok(document.children(document.root()).ok().and_then(|mut c| {
                        c.find(|&n| {
                            document
                                .node(n)
                                .is_ok_and(|n| matches!(n.kind(), NodeKind::Element(_)))
                        })
                    }))
                })
            })?;
            match root {
                Some(root) => wrap_node(agent, root),
                None => Ok(Value::NULL),
            }
        }
        (false, Some(KIND_SCROLL)) | (true, Some(KIND_VIEW)) => Ok(element),
        // A view timeline's source is its subject's scroll container,
        // known only to layout (TD-206).
        (false, Some(KIND_VIEW)) => Ok(Value::NULL),
        _ => agent.type_error("expected a ScrollTimeline receiver"),
    }
}

fn timeline_axis(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let Some(slots) = timeline_slots(agent, this)? else {
        return agent.type_error("expected a ScrollTimeline receiver");
    };
    let axis = slots.get(agent, T_AXIS)?.as_number().unwrap_or(0.0) as usize;
    let name = AXES.get(axis).map_or("block", |(name, _)| name);
    agent.string(name)
}

fn timeline_current_time(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    if let Some(source) = scroll_source(agent, this)? {
        let Some(source) = source else {
            return Ok(Value::NULL);
        };
        let controller = controller(agent)?;
        let mut controller = controller.borrow_mut();
        let id = controller.timeline_for(source);
        let progress = controller.scroll_timeline(id).and_then(|s| s.progress());
        return Ok(time_value(progress));
    }
    let Some(origin) = timeline_origin(agent, this)? else {
        return agent.type_error("expected an AnimationTimeline receiver");
    };
    let now = controller(agent)?.borrow().now();
    Ok(time_value(now.map(|now| now - origin)))
}

fn document_timeline(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    with_page(agent, |_, page| Ok(page.animations.document_timeline))
}

fn timeline_ref(agent: &mut Agent, timeline: Value) -> JsResult<TimelineRef> {
    if timeline == Value::NULL {
        return Ok(TimelineRef::None);
    }
    if let Some(source) = scroll_source(agent, timeline)? {
        let Some(source) = source else {
            return Ok(TimelineRef::None);
        };
        let id = controller(agent)?.borrow_mut().timeline_for(source);
        return Ok(TimelineRef::Scroll(rustic_style::ScrollRef::whole(id)));
    }
    if timeline_origin(agent, timeline)?.is_none() {
        return agent.type_error("expected an AnimationTimeline");
    }
    // Every DocumentTimeline shares the document's clock; a non-zero
    // origin only shifts its own currentTime (TD-203).
    Ok(TimelineRef::Document)
}

// ---- Timing ---------------------------------------------------------------

fn finite(agent: &mut Agent, value: Value, what: &str) -> JsResult<f64> {
    let number = agent.to_number(value)?;
    if !number.is_finite() {
        return agent.type_error(&format!("{what} must be finite"));
    }
    Ok(number)
}

/// Parse `<easing-function>` text.
pub(super) fn parse_easing(text: &str) -> Option<EasingFunction> {
    let parsed = parse_property(PropertyId::TransitionTimingFunction, text)?;
    match parsed.first().map(|d| &d.value) {
        Some(rustic_css::DeclaredValue::Value(PropertyValue::Easing(list)))
            if list.as_slice().len() == 1 =>
        {
            Some(list.as_slice()[0].clone())
        }
        _ => None,
    }
}

fn parse_property(property: PropertyId, value: &str) -> Option<Vec<PropertyDeclaration>> {
    parse_named(property.name(), value)
}

fn parse_named(name: &str, value: &str) -> Option<Vec<PropertyDeclaration>> {
    if value.contains(';') || value.contains('!') {
        return None;
    }
    let declaration = CssParser::default()
        .declaration(&format!("{name}: {value}"))
        .ok()?
        .value?;
    let declarations = parse_declaration(&declaration).ok()?;
    // CSS-wide keywords are not keyframe values.
    declarations
        .iter()
        .all(|d| {
            matches!(d.value, rustic_css::DeclaredValue::Value(_)) || d.property.custom().is_some()
        })
        .then_some(declarations)
}

#[derive(Default)]
struct TimingInput {
    delay: Option<f64>,
    end_delay: Option<f64>,
    fill: Option<Fill>,
    iteration_start: Option<f64>,
    iterations: Option<f64>,
    duration: Option<f64>,
    direction: Option<AnimationDirection>,
    easing: Option<EasingFunction>,
}

impl TimingInput {
    fn apply(self, timing: &mut Timing) {
        if let Some(v) = self.delay {
            timing.delay = v;
        }
        if let Some(v) = self.end_delay {
            timing.end_delay = v;
        }
        if let Some(v) = self.fill {
            timing.fill = v;
        }
        if let Some(v) = self.iteration_start {
            timing.iteration_start = v;
        }
        if let Some(v) = self.iterations {
            timing.iterations = v;
        }
        if let Some(v) = self.duration {
            timing.duration = v;
        }
        if let Some(v) = self.direction {
            timing.direction = v;
        }
        if let Some(v) = self.easing {
            timing.easing = v;
        }
    }
}

fn get(agent: &mut Agent, object: ObjectId, name: &str) -> JsResult<Value> {
    let key = agent.heap.key(name)?;
    agent.get(object, key)
}

/// `OptionalEffectTiming` / `EffectTiming` (Web Animations §6.5.4).
fn timing_input(agent: &mut Agent, value: Value) -> JsResult<TimingInput> {
    let mut out = TimingInput::default();
    if value == Value::UNDEFINED || value == Value::NULL {
        return Ok(out);
    }
    let Some(object) = value.as_object() else {
        // `unrestricted double` duration.
        let duration = agent.to_number(value)?;
        if duration.is_nan() || duration < 0.0 {
            return Err(agent
                .type_error::<()>("duration must not be negative")
                .unwrap_err());
        }
        out.duration = Some(duration);
        return Ok(out);
    };
    // Dictionary members in lexicographic order.
    let delay = get(agent, object, "delay")?;
    if delay != Value::UNDEFINED {
        out.delay = Some(finite(agent, delay, "delay")?);
    }
    let direction = get(agent, object, "direction")?;
    if direction != Value::UNDEFINED {
        let text = to_usv_string(agent, direction)?;
        out.direction = Some(match text.as_str() {
            "normal" => AnimationDirection::Normal,
            "reverse" => AnimationDirection::Reverse,
            "alternate" => AnimationDirection::Alternate,
            "alternate-reverse" => AnimationDirection::AlternateReverse,
            _ => return agent.type_error("invalid playback direction"),
        });
    }
    let duration = get(agent, object, "duration")?;
    if duration != Value::UNDEFINED {
        if duration.as_number().is_none() && to_usv_string(agent, duration)? == "auto" {
            out.duration = Some(0.0);
        } else {
            let number = agent.to_number(duration)?;
            if number.is_nan() || number < 0.0 {
                return agent.type_error("duration must be a nonnegative number or 'auto'");
            }
            out.duration = Some(number);
        }
    }
    let easing = get(agent, object, "easing")?;
    if easing != Value::UNDEFINED {
        let text = to_usv_string(agent, easing)?;
        match parse_easing(&text) {
            Some(easing) => out.easing = Some(easing),
            None => return agent.type_error("invalid easing"),
        }
    }
    let end_delay = get(agent, object, "endDelay")?;
    if end_delay != Value::UNDEFINED {
        out.end_delay = Some(finite(agent, end_delay, "endDelay")?);
    }
    let fill = get(agent, object, "fill")?;
    if fill != Value::UNDEFINED {
        let text = to_usv_string(agent, fill)?;
        out.fill = Some(match text.as_str() {
            "none" => Fill::None,
            "forwards" => Fill::Forwards,
            "backwards" => Fill::Backwards,
            "both" => Fill::Both,
            "auto" => Fill::Auto,
            _ => return agent.type_error("invalid fill mode"),
        });
    }
    let start = get(agent, object, "iterationStart")?;
    if start != Value::UNDEFINED {
        let number = finite(agent, start, "iterationStart")?;
        if number < 0.0 {
            return agent.type_error("iterationStart must not be negative");
        }
        out.iteration_start = Some(number);
    }
    let iterations = get(agent, object, "iterations")?;
    if iterations != Value::UNDEFINED {
        let number = agent.to_number(iterations)?;
        if number.is_nan() || number < 0.0 {
            return agent.type_error("iterations must be a nonnegative number");
        }
        out.iterations = Some(number);
    }
    Ok(out)
}

fn timing_object(agent: &mut Agent, timing: &Timing, computed: bool) -> JsResult<Value> {
    let object = agent.new_object()?;
    let value = Value::object(object);
    with_root(agent, value, |agent| {
        let fill = if computed && timing.fill == Fill::Auto {
            "none"
        } else {
            timing.fill.name()
        };
        let duration = if !computed && timing.duration == 0.0 {
            agent.string("auto")?
        } else {
            Value::number(timing.duration)
        };
        let direction = agent.string(direction_name(timing.direction))?;
        let easing = agent.string(&timing.easing.to_string())?;
        let fill = agent.string(fill)?;
        for (name, value) in [
            ("delay", Value::number(timing.delay)),
            ("direction", direction),
            ("duration", duration),
            ("easing", easing),
            ("endDelay", Value::number(timing.end_delay)),
            ("fill", fill),
            ("iterationStart", Value::number(timing.iteration_start)),
            ("iterations", Value::number(timing.iterations)),
        ] {
            let key = agent.heap.key(name)?;
            agent.create_data_property_or_throw(object, key, value)?;
        }
        Ok(value)
    })
}

fn direction_name(direction: AnimationDirection) -> &'static str {
    match direction {
        AnimationDirection::Normal => "normal",
        AnimationDirection::Reverse => "reverse",
        AnimationDirection::Alternate => "alternate",
        AnimationDirection::AlternateReverse => "alternate-reverse",
    }
}

// ---- Effects ---------------------------------------------------------------

fn effect_serial(agent: &mut Agent, value: Value) -> JsResult<Option<u64>> {
    let slots = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.animations.effects, value))
    })?;
    match PrivateArray::from_value(slots) {
        Some(slots) => Ok(slots.get(agent, E_SERIAL)?.as_number().map(|n| n as u64)),
        None => Ok(None),
    }
}

fn require_effect(agent: &mut Agent, value: Value) -> JsResult<u64> {
    match effect_serial(agent, value)? {
        Some(serial) => Ok(serial),
        None => agent.type_error("expected a KeyframeEffect"),
    }
}

/// Run `f` on the native effect behind a serial, wherever it lives.
fn with_effect<T>(
    agent: &mut Agent,
    serial: u64,
    f: impl FnOnce(&mut KeyframeEffect) -> T,
) -> JsResult<Option<T>> {
    with_page(agent, |_, page| {
        let homes = page.animations.homes.borrow();
        Ok(match homes.get(&serial) {
            Some(EffectHome::Attached(id)) => {
                let id = *id;
                drop(homes);
                let mut controller = page.animations.controller.borrow_mut();
                let out = controller.effect_mut(id).map(f);
                controller.effect_changed(id);
                out
            }
            Some(EffectHome::Detached(_)) => {
                drop(homes);
                let mut homes = page.animations.homes.borrow_mut();
                match homes.get_mut(&serial) {
                    Some(EffectHome::Detached(effect)) => Some(f(effect)),
                    _ => None,
                }
            }
            None => None,
        })
    })
}

fn new_effect_object(agent: &mut Agent, home: EffectHome) -> JsResult<Value> {
    let proto = prototype(agent, KEYFRAME_EFFECT)?;
    effect_object_with(agent, proto, home)
}

fn effect_object_with(agent: &mut Agent, proto: ObjectId, home: EffectHome) -> JsResult<Value> {
    let object = Value::object(agent.new_object_with(Some(proto))?);
    with_root(agent, object, |agent| {
        let serial = with_page(agent, |_, page| {
            let serial = page.animations.next_serial.get();
            page.animations.next_serial.set(serial + 1);
            if let EffectHome::Attached(id) = home {
                page.animations.effect_of.borrow_mut().insert(id, serial);
            }
            page.animations.homes.borrow_mut().insert(serial, home);
            Ok(serial)
        })?;
        let slots = PrivateArray::new(agent, &[Value::number(serial as f64)])?;
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.animations.effects, object, slots.value())
        })?;
        Ok(object)
    })
}

fn element_id(agent: &mut Agent, value: Value) -> JsResult<NodeId> {
    let id = with_page(agent, |agent, page| {
        let Ok(wrappers) = page.wrappers.try_borrow() else {
            return agent.type_error("DOM bindings are busy");
        };
        identity(agent, &wrappers, value, WrapperKind::Node)
    })?;
    let is_element = with_page(agent, |agent, page| {
        page.read(agent, id.document(), |_, document| {
            Ok(document
                .node(id)
                .is_ok_and(|node| matches!(node.kind(), NodeKind::Element(_))))
        })
    })?;
    if !is_element {
        return agent.type_error("expected an Element");
    }
    Ok(id)
}

fn wrap_node(agent: &mut Agent, node: NodeId) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        let Ok(mut wrappers) = page.wrappers.try_borrow_mut() else {
            return agent.type_error("DOM bindings are busy");
        };
        page.read(agent, node.document(), |agent, document| {
            if document.node(node).is_err() {
                return Ok(Value::NULL);
            }
            wrap(&mut wrappers, agent, document, node)
        })
    })
}

/// camelCase IDL attribute name → CSS property name (`cssFloat` → `float`).
fn css_name_of(attribute: &str) -> Option<String> {
    if attribute.starts_with("--") {
        return Some(attribute.to_string());
    }
    if attribute == "cssFloat" {
        return Some("float".into());
    }
    if attribute.contains('-') || attribute == "float" {
        return None;
    }
    let mut out = String::new();
    for c in attribute.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// CSS property name → camelCase IDL attribute name.
fn attribute_of(name: &str) -> String {
    if name.starts_with("--") {
        return name.to_string();
    }
    if name == "float" {
        return "cssFloat".into();
    }
    let mut out = String::new();
    let mut upper = false;
    for c in name.chars() {
        if c == '-' {
            upper = true;
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn composite_of(agent: &mut Agent, value: Value) -> JsResult<Option<CompositeOperation>> {
    if value == Value::UNDEFINED {
        return Ok(None);
    }
    let text = to_usv_string(agent, value)?;
    Ok(Some(match text.as_str() {
        "replace" => CompositeOperation::Replace,
        "add" => CompositeOperation::Add,
        "accumulate" => CompositeOperation::Accumulate,
        "auto" => return Ok(None),
        _ => return agent.type_error("invalid composite operation"),
    }))
}

fn composite_name(op: CompositeOperation) -> &'static str {
    match op {
        CompositeOperation::Replace => "replace",
        CompositeOperation::Add => "add",
        CompositeOperation::Accumulate => "accumulate",
    }
}

/// Animatable property members of a keyframe object, in code-point order.
fn property_members(agent: &mut Agent, object: ObjectId) -> JsResult<Vec<(String, String)>> {
    let keys = agent.own_property_keys(object)?;
    let mut names = Vec::new();
    for key in keys {
        let PropertyKey::String(id) = key else {
            continue;
        };
        let name: String =
            String::from_utf16_lossy(&agent.heap.string(id).units().collect::<Vec<_>>());
        if matches!(name.as_str(), "offset" | "easing" | "composite") {
            continue;
        }
        let Some(css) = css_name_of(&name) else {
            continue;
        };
        if !css.starts_with("--") && PropertyId::from_name(&css).is_none() && !is_shorthand(&css) {
            continue;
        }
        names.push((name, css));
    }
    names.sort();
    Ok(names)
}

fn is_shorthand(name: &str) -> bool {
    rustic_css::longhands_of(name).is_some_and(|longhands| longhands.len() > 1)
}

struct RawKeyframe {
    offset: Option<f64>,
    easing: EasingFunction,
    composite: Option<CompositeOperation>,
    values: Vec<(String, String)>,
}

fn keyframe_string(agent: &mut Agent, value: Value) -> JsResult<String> {
    to_usv_string(agent, value)
}

fn optional_offset(agent: &mut Agent, value: Value) -> JsResult<Option<f64>> {
    if value == Value::UNDEFINED || value == Value::NULL {
        return Ok(None);
    }
    let number = finite(agent, value, "offset")?;
    Ok(Some(number))
}

fn easing_value(agent: &mut Agent, value: Value) -> JsResult<EasingFunction> {
    if value == Value::UNDEFINED {
        return Ok(EasingFunction::LINEAR);
    }
    let text = to_usv_string(agent, value)?;
    match parse_easing(&text) {
        Some(easing) => Ok(easing),
        None => agent.type_error("invalid easing"),
    }
}

/// "Process a keyframes argument" (Web Animations §5.3.2).
fn process_keyframes(agent: &mut Agent, value: Value) -> JsResult<Vec<EffectKeyframe>> {
    let mut raw: Vec<RawKeyframe> = Vec::new();
    if value == Value::NULL || value == Value::UNDEFINED {
        return Ok(Vec::new());
    }
    let Some(object) = value.as_object() else {
        return agent.type_error("keyframes must be an object");
    };
    let iterator = PropertyKey::Symbol(agent.symbols.iterator);
    let method = agent.get_method(value, iterator)?;
    if let Some(method) = method {
        let items = collect_rooted(agent, |agent, each| {
            agent.iterate_values_with_method(value, method, each)
        })?;
        let frames = keyframe_objects(agent, &items);
        for (_, root) in items {
            agent.unroot(root);
        }
        raw = frames?;
    } else {
        // A property-indexed keyframe.
        let members = property_members(agent, object)?;
        let mut columns: Vec<(String, Vec<String>)> = Vec::new();
        for (attribute, css) in members {
            let v = get(agent, object, &attribute)?;
            let list = string_list(agent, v)?;
            columns.push((css, list));
        }
        let offset = get(agent, object, "offset")?;
        let offsets = optional_list(agent, offset, optional_offset)?;
        let easing = get(agent, object, "easing")?;
        let easings = optional_list(agent, easing, easing_value)?;
        let composite = get(agent, object, "composite")?;
        let composites = optional_list(agent, composite, composite_of)?;
        let mut by_offset: Vec<(f64, usize, RawKeyframe)> = Vec::new();
        for (css, values) in columns {
            let n = values.len();
            for (i, v) in values.into_iter().enumerate() {
                let computed = if n == 1 {
                    1.0
                } else {
                    i as f64 / (n - 1) as f64
                };
                match by_offset.iter_mut().find(|(o, _, _)| *o == computed) {
                    Some((_, _, frame)) => frame.values.push((css.clone(), v)),
                    None => by_offset.push((
                        computed,
                        by_offset.len(),
                        RawKeyframe {
                            offset: None,
                            easing: EasingFunction::LINEAR,
                            composite: None,
                            values: vec![(css.clone(), v)],
                        },
                    )),
                }
            }
        }
        by_offset.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let count = by_offset.len();
        for (i, (_, _, mut frame)) in by_offset.into_iter().enumerate() {
            if let Some(Some(offset)) = offsets.get(i) {
                frame.offset = Some(*offset);
            } else if offsets.len() == 1 && count == 1 {
                frame.offset = offsets[0];
            }
            if !easings.is_empty() {
                frame.easing = easings[i % easings.len()].clone();
            }
            if !composites.is_empty() {
                frame.composite = composites[i % composites.len()];
            }
            raw.push(frame);
        }
    }
    // Offsets must be in 0..=1 and loosely sorted.
    let mut previous = f64::NEG_INFINITY;
    for frame in &raw {
        if let Some(offset) = frame.offset {
            if !(0.0..=1.0).contains(&offset) {
                return agent.type_error("keyframe offsets must be between 0 and 1");
            }
            if offset < previous {
                return agent.type_error("keyframe offsets must be sorted");
            }
            previous = offset;
        }
    }
    Ok(raw
        .into_iter()
        .map(|frame| {
            let declarations = frame
                .values
                .iter()
                .filter_map(|(css, v)| parse_named(css, v))
                .flatten()
                .collect();
            EffectKeyframe {
                offset: frame.offset,
                computed_offset: 0.0,
                easing: frame.easing,
                composite: frame.composite,
                values: KeyframeValues::Declarations(declarations),
            }
        })
        .collect())
}

fn collect_rooted(
    agent: &mut Agent,
    iterate: impl FnOnce(
        &mut Agent,
        &mut dyn FnMut(&mut Agent, Value) -> JsResult<bool>,
    ) -> JsResult<()>,
) -> JsResult<Vec<(Value, HostRoot)>> {
    let mut items: Vec<(Value, HostRoot)> = Vec::new();
    let result = iterate(agent, &mut |agent, item| {
        let root = agent.root(item)?;
        items.push((item, root));
        Ok(true)
    });
    if let Err(error) = result {
        for (_, root) in items {
            agent.unroot(root);
        }
        return Err(error);
    }
    Ok(items)
}

fn keyframe_objects(agent: &mut Agent, items: &[(Value, HostRoot)]) -> JsResult<Vec<RawKeyframe>> {
    let mut raw = Vec::new();
    for &(item, _) in items {
        let frame = match item.as_object() {
            Some(frame) => {
                let offset = get(agent, frame, "offset")?;
                let offset = optional_offset(agent, offset)?;
                let easing = get(agent, frame, "easing")?;
                let easing = easing_value(agent, easing)?;
                let composite = get(agent, frame, "composite")?;
                let composite = composite_of(agent, composite)?;
                let mut values = Vec::new();
                for (attribute, css) in property_members(agent, frame)? {
                    let v = get(agent, frame, &attribute)?;
                    values.push((css, keyframe_string(agent, v)?));
                }
                RawKeyframe {
                    offset,
                    easing,
                    composite,
                    values,
                }
            }
            None if item.is_nullish() => RawKeyframe {
                offset: None,
                easing: EasingFunction::LINEAR,
                composite: None,
                values: Vec::new(),
            },
            None => return agent.type_error("keyframe must be an object"),
        };
        raw.push(frame);
    }
    Ok(raw)
}

fn string_list(agent: &mut Agent, value: Value) -> JsResult<Vec<String>> {
    let mut out = Vec::new();
    let iterable = value.as_object().is_some()
        && agent
            .get_method(value, PropertyKey::Symbol(agent.symbols.iterator))?
            .is_some();
    if iterable {
        agent.iterate_values(value, |agent, item| {
            out.push(to_usv_string(agent, item)?);
            Ok(true)
        })?;
    } else {
        out.push(to_usv_string(agent, value)?);
    }
    Ok(out)
}

fn optional_list<T>(
    agent: &mut Agent,
    value: Value,
    mut convert: impl FnMut(&mut Agent, Value) -> JsResult<T>,
) -> JsResult<Vec<T>> {
    if value == Value::UNDEFINED {
        return Ok(Vec::new());
    }
    let iterable = value.as_object().is_some()
        && agent
            .get_method(value, PropertyKey::Symbol(agent.symbols.iterator))?
            .is_some();
    if !iterable {
        return Ok(vec![convert(agent, value)?]);
    }
    let items = collect_rooted(agent, |agent, each| agent.iterate_values(value, each))?;
    let converted: JsResult<Vec<T>> = items.iter().map(|&(v, _)| convert(agent, v)).collect();
    for (_, root) in items {
        agent.unroot(root);
    }
    converted
}

fn construct_effect(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("KeyframeEffect constructor requires new");
    };
    let first = required_arg(agent, args, 0)?;
    // `new KeyframeEffect(source)` copies another effect.
    if let Some(serial) = effect_serial(agent, first)? {
        let copy = with_effect(agent, serial, |effect| effect.clone())?;
        let Some(copy) = copy else {
            return agent.type_error("the source effect is gone");
        };
        let proto = agent.host_prototype_from_new_target(KEYFRAME_EFFECT, new_target)?;
        return effect_object_with(agent, proto, EffectHome::Detached(copy));
    }
    let target = if first == Value::NULL {
        None
    } else {
        Some(element_id(agent, first)?)
    };
    let keyframes = args.get(1).copied().unwrap_or(Value::UNDEFINED);
    let keyframes = process_keyframes(agent, keyframes)?;
    let options = args.get(2).copied().unwrap_or(Value::UNDEFINED);
    let input = timing_input(agent, options)?;
    let composite = match options.as_object() {
        Some(object) => {
            let value = get(agent, object, "composite")?;
            composite_of(agent, value)?
        }
        None => None,
    };
    let mut timing = Timing::default();
    input.apply(&mut timing);
    let mut effect = KeyframeEffect::new(target, keyframes, timing);
    if let Some(op) = composite {
        effect.composite = op;
    }
    let proto = agent.host_prototype_from_new_target(KEYFRAME_EFFECT, new_target)?;
    effect_object_with(agent, proto, EffectHome::Detached(effect))
}

fn this_effect(agent: &mut Agent, this: Value) -> JsResult<u64> {
    match effect_serial(agent, this)? {
        Some(serial) => Ok(serial),
        None => agent.type_error("expected a KeyframeEffect receiver"),
    }
}

fn effect_target(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let serial = this_effect(agent, this)?;
    match with_effect(agent, serial, |e| e.target)?.flatten() {
        Some(node) => wrap_node(agent, node),
        None => Ok(Value::NULL),
    }
}

fn set_effect_target(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let serial = this_effect(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let target = if value.is_nullish() {
        None
    } else {
        Some(element_id(agent, value)?)
    };
    with_effect(agent, serial, |e| e.target = target)?;
    changed(agent)?;
    Ok(Value::UNDEFINED)
}

fn effect_composite(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let serial = this_effect(agent, this)?;
    let op = with_effect(agent, serial, |e| e.composite)?.unwrap_or(CompositeOperation::Replace);
    agent.string(composite_name(op))
}

fn set_effect_composite(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let serial = this_effect(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let text = to_usv_string(agent, value)?;
    let op = match text.as_str() {
        "replace" => CompositeOperation::Replace,
        "add" => CompositeOperation::Add,
        "accumulate" => CompositeOperation::Accumulate,
        // An invalid enumeration value is ignored by the setter.
        _ => return Ok(Value::UNDEFINED),
    };
    with_effect(agent, serial, |e| e.composite = op)?;
    changed(agent)?;
    Ok(Value::UNDEFINED)
}

fn get_keyframes(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let serial = this_effect(agent, this)?;
    let frames = with_effect(agent, serial, |e| e.keyframes.clone())?.unwrap_or_default();
    let mut objects = Vec::new();
    for frame in &frames {
        let object = agent.new_object()?;
        let value = Value::object(object);
        agent.root(value)?;
        objects.push(value);
        let offset = time_value(frame.offset);
        let easing = agent.string(&frame.easing.to_string())?;
        let composite = agent.string(frame.composite.map_or("auto", composite_name))?;
        for (name, v) in [
            ("offset", offset),
            ("computedOffset", Value::number(frame.computed_offset)),
            ("easing", easing),
            ("composite", composite),
        ] {
            let key = agent.heap.key(name)?;
            agent.create_data_property_or_throw(object, key, v)?;
        }
        let mut members: Vec<(String, String)> = match &frame.values {
            KeyframeValues::Declarations(declarations) => declarations
                .iter()
                .map(|d| {
                    let name = d
                        .property
                        .custom()
                        .map(str::to_string)
                        .or_else(|| d.property.longhand().map(|p| p.name().to_string()))
                        .unwrap_or_default();
                    (attribute_of(&name), d.value.to_string())
                })
                .collect(),
            KeyframeValues::Computed(values) => values
                .iter()
                .map(|(p, v)| (attribute_of(p.name()), v.to_string()))
                .collect(),
        };
        members.sort();
        for (name, text) in members {
            let key = agent.heap.key(&name)?;
            let v = agent.string(&text)?;
            agent.create_data_property_or_throw(object, key, v)?;
        }
    }
    let array = agent.create_array_from_list(&objects)?;
    Ok(Value::object(array))
}

fn set_keyframes(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let serial = this_effect(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let mut frames = process_keyframes(agent, value)?;
    rustic_style::distribute_offsets(&mut frames);
    with_effect(agent, serial, |e| e.keyframes = frames)?;
    changed(agent)?;
    Ok(Value::UNDEFINED)
}

fn get_timing(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let serial = this_effect(agent, this)?;
    let timing = with_effect(agent, serial, |e| e.timing.clone())?.unwrap_or_default();
    timing_object(agent, &timing, false)
}

fn get_computed_timing(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let serial = this_effect(agent, this)?;
    let attached = with_page(agent, |_, page| {
        Ok(match page.animations.homes.borrow().get(&serial) {
            Some(EffectHome::Attached(id)) => Some(*id),
            _ => None,
        })
    })?;
    let timing = with_effect(agent, serial, |e| e.timing.clone())?.unwrap_or_default();
    let computed = match attached {
        Some(id) => controller(agent)?.borrow().computed_timing(id),
        None => None,
    }
    .unwrap_or_else(|| timing.compute(None, false));
    let object = timing_object(agent, &timing, true)?;
    let target = object.as_object().expect("timing object");
    with_root(agent, object, |agent| {
        for (name, v) in [
            ("endTime", Value::number(computed.end_time)),
            ("activeDuration", Value::number(computed.active_duration)),
            ("localTime", time_value(computed.local_time)),
            ("progress", time_value(computed.progress)),
            ("currentIteration", time_value(computed.current_iteration)),
        ] {
            let key = agent.heap.key(name)?;
            agent.create_data_property_or_throw(target, key, v)?;
        }
        Ok(object)
    })
}

fn update_timing(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let serial = this_effect(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    if value.as_object().is_none() && !value.is_nullish() {
        return agent.type_error("timing must be a dictionary");
    }
    let input = timing_input(agent, value)?;
    with_effect(agent, serial, |e| input.apply(&mut e.timing))?;
    changed(agent)?;
    Ok(Value::UNDEFINED)
}

// ---- Animations -------------------------------------------------------------

fn slots_of(agent: &mut Agent, this: Value) -> JsResult<PrivateArray> {
    let slots = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.animations.states, this))
    })?;
    match PrivateArray::from_value(slots) {
        Some(slots) => Ok(slots),
        None => agent.type_error("expected an Animation receiver"),
    }
}

fn id_of(agent: &mut Agent, this: Value) -> JsResult<(PrivateArray, AnimationId)> {
    let slots = slots_of(agent, this)?;
    let id = slots.get(agent, A_ID)?.as_number().unwrap_or(-1.0);
    Ok((slots, id as AnimationId))
}

/// The wrapper for an animation, created on first use.
pub(super) fn wrapper(agent: &mut Agent, id: AnimationId) -> JsResult<Value> {
    if let Some(existing) = with_page(agent, |_, page| {
        Ok(page.animations.wrappers.borrow().get(&id).map(|(v, _)| *v))
    })? {
        return Ok(existing);
    }
    let owner = controller(agent)?.borrow().get(id).map(|a| a.owner.clone());
    let interface = match owner {
        Some(Owner::CssAnimation { .. }) => CSS_ANIMATION,
        Some(Owner::CssTransition { .. }) => CSS_TRANSITION,
        _ => ANIMATION,
    };
    let proto = prototype(agent, interface)?;
    create_wrapper(agent, proto, id)
}

fn create_wrapper(agent: &mut Agent, proto: ObjectId, id: AnimationId) -> JsResult<Value> {
    let object = Value::object(agent.new_object_with(Some(proto))?);
    let root = agent.root(object)?;
    let result = (|| {
        events::register_target(agent, object)?;
        let timeline = with_page(agent, |_, page| Ok(page.animations.document_timeline))?;
        let has_timeline = !matches!(
            controller(agent)?.borrow().get(id).map(|a| &a.timeline),
            Some(TimelineRef::None)
        );
        let empty = agent.string("")?;
        let slots = PrivateArray::new(
            agent,
            &[
                Value::number(id as f64),
                Value::NULL,
                if has_timeline { timeline } else { Value::NULL },
                Value::UNDEFINED,
                Value::UNDEFINED,
                empty,
            ],
        )?;
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.animations.states, object, slots.value())
        })?;
        // The effect object, for an animation that has one.
        let has_effect = controller(agent)?
            .borrow()
            .get(id)
            .is_some_and(|a| a.effect.is_some());
        if has_effect {
            let effect = new_effect_object(agent, EffectHome::Attached(id))?;
            slots.set(agent, A_EFFECT, effect)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            with_page(agent, |_, page| {
                page.animations
                    .wrappers
                    .borrow_mut()
                    .insert(id, (object, root));
                Ok(())
            })?;
            Ok(object)
        }
        Err(error) => {
            agent.unroot(root);
            Err(error)
        }
    }
}

fn construct_animation(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(new_target) = agent.new_target() else {
        return agent.type_error("Animation constructor requires new");
    };
    let effect = args.first().copied().unwrap_or(Value::UNDEFINED);
    let serial = if effect.is_nullish() {
        None
    } else {
        Some(require_effect(agent, effect)?)
    };
    let timeline_arg = args.get(1).copied().unwrap_or(Value::UNDEFINED);
    let timeline_value = if timeline_arg == Value::UNDEFINED {
        with_page(agent, |_, page| Ok(page.animations.document_timeline))?
    } else {
        timeline_arg
    };
    let timeline = timeline_ref(agent, timeline_value)?;
    let native = match serial {
        Some(serial) => Some(take_effect(agent, serial)?),
        None => None,
    };
    let id = controller(agent)?.borrow_mut().create(native, timeline);
    let proto = agent.host_prototype_from_new_target(ANIMATION, new_target)?;
    let object = create_wrapper(agent, proto, id)?;
    let slots = slots_of(agent, object)?;
    slots.set(agent, A_TIMELINE, timeline_value)?;
    if let Some(serial) = serial {
        attach_effect(agent, serial, id)?;
        slots.set(agent, A_EFFECT, effect)?;
    }
    Ok(object)
}

/// Detach the native effect behind `serial` from wherever it is.
fn take_effect(agent: &mut Agent, serial: u64) -> JsResult<KeyframeEffect> {
    let home = with_page(agent, |_, page| {
        Ok(page.animations.homes.borrow_mut().remove(&serial))
    })?;
    match home {
        Some(EffectHome::Detached(effect)) => Ok(effect),
        Some(EffectHome::Attached(previous)) => {
            let c = controller(agent)?;
            let effect = c.borrow().get(previous).and_then(|a| a.effect.clone());
            c.borrow_mut().set_effect(previous, None);
            with_page(agent, |_, page| {
                page.animations.effect_of.borrow_mut().remove(&previous);
                Ok(())
            })?;
            // The previous animation's wrapper no longer has an effect.
            if let Some(old) = with_page(agent, |_, page| {
                Ok(page
                    .animations
                    .wrappers
                    .borrow()
                    .get(&previous)
                    .map(|(v, _)| *v))
            })? {
                let slots = slots_of(agent, old)?;
                slots.set(agent, A_EFFECT, Value::NULL)?;
            }
            match effect {
                Some(effect) => Ok(effect),
                None => agent.type_error("the effect is gone"),
            }
        }
        None => agent.type_error("the effect is gone"),
    }
}

fn attach_effect(agent: &mut Agent, serial: u64, id: AnimationId) -> JsResult<()> {
    with_page(agent, |_, page| {
        page.animations
            .homes
            .borrow_mut()
            .insert(serial, EffectHome::Attached(id));
        page.animations.effect_of.borrow_mut().insert(id, serial);
        Ok(())
    })
}

fn animation_id(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    slots_of(agent, this)?.get(agent, A_NAME)
}

fn set_animation_id(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let slots = slots_of(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let text = to_usv_string(agent, value)?;
    let text = agent.string(&text)?;
    slots.set(agent, A_NAME, text)?;
    Ok(Value::UNDEFINED)
}

fn animation_effect(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    slots_of(agent, this)?.get(agent, A_EFFECT)
}

fn set_animation_effect(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (slots, id) = id_of(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let current = slots.get(agent, A_EFFECT)?;
    if current == value {
        return Ok(Value::UNDEFINED);
    }
    // Detach the current effect.
    if current != Value::NULL {
        if let Some(serial) = effect_serial(agent, current)? {
            let c = controller(agent)?;
            let effect = c.borrow().get(id).and_then(|a| a.effect.clone());
            c.borrow_mut().set_effect(id, None);
            with_page(agent, |_, page| {
                page.animations.effect_of.borrow_mut().remove(&id);
                if let Some(effect) = effect {
                    page.animations
                        .homes
                        .borrow_mut()
                        .insert(serial, EffectHome::Detached(effect));
                }
                Ok(())
            })?;
        }
    }
    if value.is_nullish() {
        slots.set(agent, A_EFFECT, Value::NULL)?;
    } else {
        let serial = require_effect(agent, value)?;
        let effect = take_effect(agent, serial)?;
        controller(agent)?.borrow_mut().set_effect(id, Some(effect));
        attach_effect(agent, serial, id)?;
        slots.set(agent, A_EFFECT, value)?;
    }
    changed(agent)?;
    Ok(Value::UNDEFINED)
}

fn animation_timeline(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    slots_of(agent, this)?.get(agent, A_TIMELINE)
}

fn set_animation_timeline(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (slots, id) = id_of(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::NULL);
    let value = if value == Value::UNDEFINED {
        Value::NULL
    } else {
        value
    };
    let timeline = timeline_ref(agent, value)?;
    controller(agent)?.borrow_mut().set_timeline(id, timeline);
    slots.set(agent, A_TIMELINE, value)?;
    sync(agent, this, id)?;
    changed(agent)?;
    Ok(Value::UNDEFINED)
}

fn start_time(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, id) = id_of(agent, this)?;
    let time = controller(agent)?
        .borrow()
        .get(id)
        .and_then(|a| a.start_time());
    Ok(time_value(time))
}

fn optional_time(agent: &mut Agent, value: Value) -> JsResult<Option<f64>> {
    if value.is_nullish() {
        return Ok(None);
    }
    Ok(Some(finite(agent, value, "time")?))
}

fn set_start_time(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, id) = id_of(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let time = optional_time(agent, value)?;
    controller(agent)?.borrow_mut().set_start_time(id, time);
    sync(agent, this, id)?;
    changed(agent)?;
    Ok(Value::UNDEFINED)
}

fn current_time(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, id) = id_of(agent, this)?;
    let time = controller(agent)?.borrow().current_time(id);
    Ok(time_value(time))
}

fn set_current_time(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, id) = id_of(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let time = optional_time(agent, value)?;
    let result = controller(agent)?.borrow_mut().set_current_time(id, time);
    if let Err(error) = result {
        return Err(invalid_state(agent, error));
    }
    sync(agent, this, id)?;
    changed(agent)?;
    Ok(Value::UNDEFINED)
}

fn playback_rate(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, id) = id_of(agent, this)?;
    let rate = controller(agent)?
        .borrow()
        .get(id)
        .map_or(1.0, |a| a.playback_rate());
    Ok(Value::number(rate))
}

fn set_playback_rate(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (_, id) = id_of(agent, this)?;
    let value = args.first().copied().unwrap_or(Value::UNDEFINED);
    let rate = finite(agent, value, "playbackRate")?;
    let result = controller(agent)?.borrow_mut().set_playback_rate(id, rate);
    if let Err(error) = result {
        return Err(invalid_state(agent, error));
    }
    sync(agent, this, id)?;
    changed(agent)?;
    Ok(Value::UNDEFINED)
}

fn play_state(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, id) = id_of(agent, this)?;
    let state = controller(agent)?
        .borrow()
        .play_state(id)
        .unwrap_or(PlayStateValue::Idle);
    agent.string(state.name())
}

fn replace_state(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, id) = id_of(agent, this)?;
    let state = controller(agent)?
        .borrow()
        .get(id)
        .map(|a| a.replace_state());
    agent.string(state.map_or("active", |s| s.name()))
}

fn pending(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, id) = id_of(agent, this)?;
    let pending = controller(agent)?
        .borrow()
        .get(id)
        .is_some_and(|a| a.pending());
    Ok(Value::boolean(pending))
}

fn promise_slot(agent: &mut Agent, slots: PrivateArray, slot: u32) -> JsResult<ObjectId> {
    let current = slots.get(agent, slot)?;
    if let Some(promise) = current.as_object() {
        return Ok(promise);
    }
    let promise = agent.new_host_promise()?;
    slots.set(agent, slot, Value::object(promise))?;
    Ok(promise)
}

fn ready(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (slots, id) = id_of(agent, this)?;
    let promise = promise_slot(agent, slots, A_READY)?;
    let pending = controller(agent)?
        .borrow()
        .get(id)
        .is_some_and(|a| a.pending());
    if !pending && agent.promise_rejected(promise).is_none() {
        agent.resolve_host_promise(promise, this)?;
    }
    Ok(Value::object(promise))
}

fn finished(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (slots, id) = id_of(agent, this)?;
    let promise = promise_slot(agent, slots, A_FINISHED)?;
    let state = controller(agent)?.borrow().play_state(id);
    if state == Some(PlayStateValue::Finished) && agent.promise_rejected(promise).is_none() {
        agent.resolve_host_promise(promise, this)?;
    }
    Ok(Value::object(promise))
}

/// Keep the wrapper's promises in step with the animation after a
/// synchronous operation: a new pending `ready` promise once a settled one
/// would be stale, a new `finished` promise once no longer finished, and
/// `finished` resolved when the operation finished it.
fn sync(agent: &mut Agent, this: Value, id: AnimationId) -> JsResult<()> {
    let slots = slots_of(agent, this)?;
    let (is_pending, state) = {
        let c = controller(agent)?;
        let c = c.borrow();
        (c.get(id).is_some_and(|a| a.pending()), c.play_state(id))
    };
    let ready = slots.get(agent, A_READY)?;
    if let Some(promise) = ready.as_object() {
        let settled = agent.promise_rejected(promise).is_some();
        if is_pending && settled {
            slots.set(agent, A_READY, Value::UNDEFINED)?;
        } else if !is_pending && !settled {
            agent.resolve_host_promise(promise, this)?;
        }
    }
    let finished = slots.get(agent, A_FINISHED)?;
    if let Some(promise) = finished.as_object() {
        let settled = agent.promise_rejected(promise).is_some();
        if state == Some(PlayStateValue::Finished) && !settled {
            agent.resolve_host_promise(promise, this)?;
        } else if state != Some(PlayStateValue::Finished) && settled {
            slots.set(agent, A_FINISHED, Value::UNDEFINED)?;
        }
    }
    Ok(())
}

fn reject_promises(agent: &mut Agent, this: Value) -> JsResult<()> {
    let slots = slots_of(agent, this)?;
    for slot in [A_FINISHED, A_READY] {
        if let Some(promise) = slots.get(agent, slot)?.as_object() {
            if agent.promise_rejected(promise).is_none() {
                let error =
                    match creation::exception(agent, "AbortError", "the animation was canceled") {
                        Abrupt::Throw(value) => value,
                        other => return Err(other),
                    };
                agent.mark_promise_handled(promise);
                agent.reject_host_promise(promise, error);
                slots.set(agent, slot, Value::UNDEFINED)?;
            }
        }
    }
    Ok(())
}

fn playback_method(
    agent: &mut Agent,
    this: Value,
    operation: impl FnOnce(&mut AnimationController, AnimationId) -> Result<(), AnimationError>,
) -> JsResult<Value> {
    let (_, id) = id_of(agent, this)?;
    let result = operation(&mut controller(agent)?.borrow_mut(), id);
    if let Err(error) = result {
        return Err(invalid_state(agent, error));
    }
    sync(agent, this, id)?;
    changed(agent)?;
    Ok(Value::UNDEFINED)
}

fn play(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    playback_method(agent, this, |c, id| c.play(id, true))
}

fn pause(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    playback_method(agent, this, |c, id| c.pause(id))
}

fn finish(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    playback_method(agent, this, |c, id| c.finish(id))
}

fn reverse(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    playback_method(agent, this, |c, id| c.reverse(id))
}

fn update_playback_rate(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let value = required_arg(agent, args, 0)?;
    let rate = finite(agent, value, "playbackRate")?;
    playback_method(agent, this, |c, id| c.update_playback_rate(id, rate))
}

fn persist(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    playback_method(agent, this, |c, id| {
        c.persist(id);
        Ok(())
    })
}

fn cancel(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, id) = id_of(agent, this)?;
    let was_idle = controller(agent)?.borrow().play_state(id) == Some(PlayStateValue::Idle);
    controller(agent)?.borrow_mut().cancel(id);
    if !was_idle {
        reject_promises(agent, this)?;
    }
    changed(agent)?;
    Ok(Value::UNDEFINED)
}

fn commit_styles(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    // Writing animated values into the style attribute needs the
    // animated computed values of one effect (TD-203).
    Err(creation::exception(
        agent,
        "NotSupportedError",
        "commitStyles() is not supported yet",
    ))
}

fn css_name(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, id) = id_of(agent, this)?;
    let owner = controller(agent)?.borrow().get(id).map(|a| a.owner.clone());
    match owner {
        Some(Owner::CssAnimation { name, .. }) => agent.string(&name),
        Some(Owner::CssTransition { property, .. }) => agent.string(property.name()),
        _ => agent.string(""),
    }
}

// ---- Element.animate() and getAnimations() ---------------------------------

fn animate(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let target = element_id(agent, this)?;
    let keyframes = args.first().copied().unwrap_or(Value::UNDEFINED);
    let frames = process_keyframes(agent, keyframes)?;
    let options = args.get(1).copied().unwrap_or(Value::UNDEFINED);
    let input = timing_input(agent, options)?;
    let mut composite = None;
    let mut name = None;
    let mut timeline = None;
    if let Some(object) = options.as_object() {
        let value = get(agent, object, "composite")?;
        composite = composite_of(agent, value)?;
        let value = get(agent, object, "id")?;
        if value != Value::UNDEFINED {
            name = Some(to_usv_string(agent, value)?);
        }
        let value = get(agent, object, "timeline")?;
        if value != Value::UNDEFINED {
            timeline = Some((value, timeline_ref(agent, value)?));
        }
    }
    let mut timing = Timing::default();
    input.apply(&mut timing);
    let mut effect = KeyframeEffect::new(Some(target), frames, timing);
    if let Some(op) = composite {
        effect.composite = op;
    }
    let timeline_ref = timeline
        .as_ref()
        .map_or(TimelineRef::Document, |(_, r)| r.clone());
    let id = controller(agent)?
        .borrow_mut()
        .create(Some(effect), timeline_ref);
    let wrapper = wrapper(agent, id)?;
    if let Some((value, _)) = timeline {
        let slots = slots_of(agent, wrapper)?;
        slots.set(agent, A_TIMELINE, value)?;
    }
    if let Some(name) = name {
        let slots = slots_of(agent, wrapper)?;
        let text = agent.string(&name)?;
        slots.set(agent, A_NAME, text)?;
    }
    let result = controller(agent)?.borrow_mut().play(id, true);
    if let Err(error) = result {
        return Err(invalid_state(agent, error));
    }
    changed(agent)?;
    Ok(wrapper)
}

fn subtree_of(agent: &mut Agent, root: NodeId) -> JsResult<HashSet<NodeId>> {
    with_page(agent, |agent, page| {
        page.read(agent, root.document(), |_, document| {
            let mut out = HashSet::new();
            if let Ok(walk) = document.walk(root) {
                for (id, _) in walk {
                    out.insert(id);
                }
            }
            Ok(out)
        })
    })
}

fn animations_array(agent: &mut Agent, ids: Vec<AnimationId>) -> JsResult<Value> {
    let mut values = Vec::new();
    for id in ids {
        values.push(wrapper(agent, id)?);
    }
    Ok(Value::object(agent.create_array_from_list(&values)?))
}

fn element_get_animations(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let target = element_id(agent, this)?;
    let mut subtree = false;
    if let Some(object) = args.first().and_then(|v| v.as_object()) {
        let value = get(agent, object, "subtree")?;
        subtree = agent.to_boolean(value);
    }
    let nodes = if subtree {
        subtree_of(agent, target)?
    } else {
        HashSet::from([target])
    };
    let ids = controller(agent)?.borrow().animations_of(&nodes);
    animations_array(agent, ids)
}

fn document_get_animations(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let document = with_page(agent, |agent, page| {
        let Ok(wrappers) = page.wrappers.try_borrow() else {
            return agent.type_error("DOM bindings are busy");
        };
        identity(agent, &wrappers, this, WrapperKind::Node)
    })?;
    let nodes = subtree_of(agent, document)?;
    let ids = controller(agent)?.borrow().animations_of(&nodes);
    animations_array(agent, ids)
}

// ---- The rendering step ------------------------------------------------------

/// "Update animations and send events" (Web Animations §4.5): advance the
/// document timeline to `now` (milliseconds), then dispatch the CSS
/// transition and animation events and the playback events, and settle
/// the promises, in the order they were queued.
pub(super) fn update(agent: &mut Agent, now: f64) -> JsResult<()> {
    let c = controller(agent)?;
    let events = {
        let mut c = c.borrow_mut();
        c.tick(now);
        c.take_events()
    };
    if c.borrow().animations().next().is_some() || !events.is_empty() {
        changed(agent)?;
    }
    for event in events {
        let existing = with_page(agent, |_, page| {
            Ok(page
                .animations
                .wrappers
                .borrow()
                .get(&event.animation)
                .map(|(v, _)| *v))
        })?;
        match event.kind {
            AnimationEventKind::TransitionRun
            | AnimationEventKind::TransitionStart
            | AnimationEventKind::TransitionEnd
            | AnimationEventKind::TransitionCancel => {
                if let Some(node) = event.target {
                    events::fire_css(
                        agent,
                        events::TRANSITION_EVENT,
                        node,
                        event.kind.event_type(),
                        &event.name,
                        event.elapsed_time,
                    )?;
                }
            }
            AnimationEventKind::AnimationStart
            | AnimationEventKind::AnimationIteration
            | AnimationEventKind::AnimationEnd
            | AnimationEventKind::AnimationCancel => {
                if let Some(node) = event.target {
                    events::fire_css(
                        agent,
                        events::ANIMATION_EVENT,
                        node,
                        event.kind.event_type(),
                        &event.name,
                        event.elapsed_time,
                    )?;
                }
            }
            AnimationEventKind::Ready => {
                if let Some(wrapper) = existing {
                    let slots = slots_of(agent, wrapper)?;
                    if let Some(promise) = slots.get(agent, A_READY)?.as_object() {
                        if agent.promise_rejected(promise).is_none() {
                            agent.resolve_host_promise(promise, wrapper)?;
                        }
                    }
                }
            }
            AnimationEventKind::Unfinish => {
                if let Some(wrapper) = existing {
                    let slots = slots_of(agent, wrapper)?;
                    if let Some(promise) = slots.get(agent, A_FINISHED)?.as_object() {
                        if agent.promise_rejected(promise).is_some() {
                            slots.set(agent, A_FINISHED, Value::UNDEFINED)?;
                        }
                    }
                }
            }
            AnimationEventKind::Finish
            | AnimationEventKind::Cancel
            | AnimationEventKind::Remove => {
                let Some(wrapper) = existing else {
                    continue;
                };
                with_root(agent, wrapper, |agent| {
                    match event.kind {
                        AnimationEventKind::Finish => {
                            let slots = slots_of(agent, wrapper)?;
                            if let Some(promise) = slots.get(agent, A_FINISHED)?.as_object() {
                                if agent.promise_rejected(promise).is_none() {
                                    agent.resolve_host_promise(promise, wrapper)?;
                                }
                            }
                        }
                        AnimationEventKind::Cancel => reject_promises(agent, wrapper)?,
                        _ => {}
                    }
                    events::fire_playback(
                        agent,
                        wrapper,
                        event.kind.event_type(),
                        event.current_time,
                        event.timeline_time,
                    )
                })?;
            }
        }
    }
    release(agent)
}

/// Unroot wrappers of animations the controller dropped, and drop the
/// oldest finished script animations past [`KEPT_FINISHED`].
fn release(agent: &mut Agent) -> JsResult<()> {
    let c = controller(agent)?;
    let mut gone = Vec::new();
    with_page(agent, |_, page| {
        let controller = c.borrow();
        let mut finished = page.animations.finished.borrow_mut();
        for a in controller.animations() {
            if matches!(a.owner, Owner::Script)
                && controller.play_state(a.id) == Some(PlayStateValue::Finished)
                && !controller.is_relevant(a.id)
                && !finished.contains(&a.id)
            {
                finished.push(a.id);
            }
        }
        finished.retain(|id| controller.play_state(*id) == Some(PlayStateValue::Finished));
        let excess = finished.len().saturating_sub(KEPT_FINISHED);
        let dropped: Vec<AnimationId> = finished.drain(..excess).collect();
        drop(controller);
        for id in &dropped {
            c.borrow_mut().forget(*id);
        }
        let controller = c.borrow();
        let wrappers = page.animations.wrappers.borrow();
        gone.extend(
            wrappers
                .iter()
                .filter(|(id, _)| controller.get(**id).is_none())
                .map(|(id, (_, root))| (*id, *root)),
        );
        Ok(())
    })?;
    for (id, root) in gone {
        agent.unroot(root);
        with_page(agent, |_, page| {
            page.animations.wrappers.borrow_mut().remove(&id);
            if let Some(serial) = page.animations.effect_of.borrow_mut().remove(&id) {
                page.animations.homes.borrow_mut().remove(&serial);
            }
            Ok(())
        })?;
    }
    Ok(())
}
