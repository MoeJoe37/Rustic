//! Values of the transition and animation properties: CSS Transitions
//! Level 1 and 2 §2 (`transition-*`), CSS Animations Level 1 and 2 §4
//! (`animation-*`) and the `animation-timeline` values of Scroll-driven
//! Animations §2–3, with the `transition` and `animation` shorthands
//! (editor's drafts reviewed 2026-11-25).
//!
//! Every property here is a comma-separated list. Times keep the unit they
//! were written in (`500ms` serializes as written); computed values use
//! seconds, as CSSOM requires.

use std::fmt;
use std::sync::Arc;

use crate::background::write_list;
use crate::color::write_number;
use crate::properties::{ident, keywords, number};
use crate::{
    ComponentValue, CssWideKeyword, EasingFunction, EasingList, LengthPercentageOrAuto, PropertyId,
    PropertyValue, Token, Value,
};

/// A `<time>`, kept in the unit it was written in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Time {
    /// The time in seconds.
    pub seconds: f64,
    /// Whether it was written in milliseconds (for serialization only).
    pub milliseconds: bool,
}

impl Time {
    /// Zero seconds.
    pub const ZERO: Self = Self {
        seconds: 0.0,
        milliseconds: false,
    };

    /// `seconds` written in seconds.
    pub const fn seconds(seconds: f64) -> Self {
        Self {
            seconds,
            milliseconds: false,
        }
    }

    /// The time in milliseconds.
    pub fn as_millis(self) -> f64 {
        self.seconds * 1000.0
    }

    /// The computed form: the same time in seconds.
    pub fn computed(self) -> Self {
        Self::seconds(self.seconds)
    }

    /// Parse a `<time>` component value (a `s` or `ms` dimension, or a
    /// math function of times). A unitless zero is not a time.
    pub fn parse(value: &Value) -> Option<Self> {
        match &value.value {
            ComponentValue::Token(Token::Dimension(n, unit)) => {
                let unit = unit.to_ascii_lowercase();
                match unit.as_str() {
                    "s" => Some(Self::seconds(n.value)),
                    "ms" => Some(Self {
                        seconds: n.value / 1000.0,
                        milliseconds: true,
                    }),
                    _ => None,
                }
            }
            ComponentValue::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
                // calc() over times: a sum of times and products by numbers.
                calc_time(arguments).map(|seconds| Self::seconds(crate::calc::censor(seconds)))
            }
            _ => None,
        }
    }
}

/// Evaluate a small `calc()` over times: `+`/`-` between times, `*` and
/// `/` by plain numbers, and nested parentheses or `calc()`.
fn calc_time(arguments: &[Value]) -> Option<f64> {
    fn term(values: &[&Value]) -> Option<f64> {
        // factor (('*' | '/') factor)*, where exactly one factor is a time.
        let mut seconds: Option<f64> = None;
        let mut scale = 1.0;
        let mut op = '*';
        let mut expect_factor = true;
        for value in values {
            if expect_factor {
                if let Some(time) = Time::parse(value) {
                    if seconds.is_some() || op == '/' {
                        return None;
                    }
                    seconds = Some(time.seconds);
                } else if let ComponentValue::Block {
                    kind: crate::BlockKind::Paren,
                    contents,
                } = &value.value
                {
                    let inner = calc_time(contents);
                    match inner {
                        Some(t) if seconds.is_none() && op == '*' => seconds = Some(t),
                        _ => return None,
                    }
                } else {
                    let n = number(value)?;
                    if op == '*' {
                        scale *= n;
                    } else {
                        scale /= n;
                    }
                }
                expect_factor = false;
            } else {
                op = match value.token()? {
                    Token::Delim('*') => '*',
                    Token::Delim('/') => '/',
                    _ => return None,
                };
                expect_factor = true;
            }
        }
        (!expect_factor).then_some(())?;
        seconds.map(|s| s * scale)
    }
    let parts: Vec<&Value> = arguments.iter().filter(|v| !v.is_whitespace()).collect();
    let mut total = 0.0;
    let mut start = 0;
    let mut sign = 1.0;
    let mut index = 0;
    while index <= parts.len() {
        let at_end = index == parts.len();
        let operator = if at_end {
            None
        } else {
            match parts[index].token() {
                Some(Token::Delim('+')) => Some(1.0),
                Some(Token::Delim('-')) => Some(-1.0),
                _ => None,
            }
        };
        if at_end || operator.is_some() {
            if index == start {
                return None;
            }
            total += sign * term(&parts[start..index])?;
            if let Some(next) = operator {
                sign = next;
            }
            start = index + 1;
        }
        index += 1;
    }
    // A `+`/`-` written as part of a signed dimension is not an operator,
    // so a missing operator between two times fails in `term`.
    Some(total)
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.milliseconds {
            write_number(f, self.seconds * 1000.0)?;
            f.write_str("ms")
        } else {
            write_number(f, self.seconds)?;
            f.write_str("s")
        }
    }
}

/// One `transition-property` entry.
#[derive(Clone, Debug, PartialEq)]
pub enum TransitionProperty {
    /// `all`.
    All,
    /// A property name: a supported longhand or shorthand, an unknown
    /// identifier (kept, but it transitions nothing), or a custom property
    /// (case-sensitive). Non-custom names are lowercased.
    Name(Arc<str>),
}

impl TransitionProperty {
    /// The supported longhands this entry names (`all` names every
    /// longhand; unknown names and custom properties name none).
    pub fn longhands(&self) -> &'static [PropertyId] {
        match self {
            Self::All => PropertyId::ALL,
            Self::Name(name) if name.starts_with("--") => &[],
            Self::Name(name) => crate::properties::longhands_of(name).unwrap_or(&[]),
        }
    }

    /// The custom property this entry names, if any.
    pub fn custom(&self) -> Option<&str> {
        match self {
            Self::Name(name) if name.starts_with("--") => Some(name),
            _ => None,
        }
    }
}

impl fmt::Display for TransitionProperty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => f.write_str("all"),
            Self::Name(name) => crate::serialize::serialize_identifier(f, name),
        }
    }
}

keywords! {
    /// `transition-behavior` entries.
    TransitionBehavior {
        Normal => "normal",
        AllowDiscrete => "allow-discrete",
    }
}

keywords! {
    /// `animation-direction` entries.
    AnimationDirection {
        Normal => "normal",
        Reverse => "reverse",
        Alternate => "alternate",
        AlternateReverse => "alternate-reverse",
    }
}

keywords! {
    /// `animation-fill-mode` entries.
    FillMode {
        None => "none",
        Forwards => "forwards",
        Backwards => "backwards",
        Both => "both",
    }
}

keywords! {
    /// `animation-play-state` entries.
    PlayState {
        Running => "running",
        Paused => "paused",
    }
}

keywords! {
    /// `animation-composition` entries (and Web Animations composite
    /// operations).
    CompositeOperation {
        Replace => "replace",
        Add => "add",
        Accumulate => "accumulate",
    }
}

keywords! {
    /// The scroller of `scroll()`.
    Scroller {
        Nearest => "nearest",
        Root => "root",
        SelfElement => "self",
    }
}

keywords! {
    /// A scroll axis.
    ScrollAxis {
        Block => "block",
        Inline => "inline",
        X => "x",
        Y => "y",
    }
}

/// One `animation-duration` entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AnimationDuration {
    /// `auto`: zero for time-based animations, the whole range for
    /// scroll-driven ones.
    Auto,
    /// A nonnegative time.
    Time(Time),
}

impl fmt::Display for AnimationDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Time(time) => time.fmt(f),
        }
    }
}

/// One `animation-name` entry.
#[derive(Clone, Debug, PartialEq)]
pub enum AnimationName {
    /// `none`.
    None,
    /// A `<keyframes-name>`: an identifier (case-sensitive) or a string.
    Name {
        /// The name.
        name: Arc<str>,
        /// Whether it was written as a string.
        quoted: bool,
    },
}

impl AnimationName {
    /// The keyframes name, if not `none`.
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Name { name, .. } => Some(name),
        }
    }
}

impl fmt::Display for AnimationName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Name { name, quoted: true } => crate::serialize::serialize_string(f, name),
            Self::Name { name, .. } => crate::serialize::serialize_identifier(f, name),
        }
    }
}

/// One `animation-iteration-count` entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IterationCount {
    /// `infinite`.
    Infinite,
    /// A nonnegative number.
    Number(f64),
}

impl IterationCount {
    /// The count as a float (`infinite` is infinity).
    pub fn value(self) -> f64 {
        match self {
            Self::Infinite => f64::INFINITY,
            Self::Number(n) => n,
        }
    }
}

impl fmt::Display for IterationCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Infinite => f.write_str("infinite"),
            Self::Number(n) => write_number(f, *n),
        }
    }
}

/// One `animation-timeline` entry.
#[derive(Clone, Debug, PartialEq)]
pub enum AnimationTimeline {
    /// `auto`: the document timeline.
    Auto,
    /// `none`: no timeline (the animation is inactive).
    None,
    /// A named scroll or view timeline (`<dashed-ident>`).
    Named(Arc<str>),
    /// `scroll(<scroller>? <axis>?)`.
    Scroll(Scroller, ScrollAxis),
    /// `view(<axis>? <inset>?)`: the axis and the start and end insets.
    View(ScrollAxis, LengthPercentageOrAuto, LengthPercentageOrAuto),
}

impl fmt::Display for AnimationTimeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::None => f.write_str("none"),
            Self::Named(name) => crate::serialize::serialize_identifier(f, name),
            Self::Scroll(scroller, axis) => {
                f.write_str("scroll(")?;
                let mut parts = Vec::new();
                if *scroller != Scroller::Nearest {
                    parts.push(scroller.name());
                }
                if *axis != ScrollAxis::Block {
                    parts.push(axis.name());
                }
                f.write_str(&parts.join(" "))?;
                f.write_str(")")
            }
            Self::View(axis, start, end) => {
                f.write_str("view(")?;
                let mut wrote = false;
                if *axis != ScrollAxis::Block {
                    f.write_str(axis.name())?;
                    wrote = true;
                }
                let auto = LengthPercentageOrAuto::Auto;
                if *start != auto || *end != auto {
                    if wrote {
                        f.write_str(" ")?;
                    }
                    write!(f, "{start}")?;
                    if end != start {
                        write!(f, " {end}")?;
                    }
                }
                f.write_str(")")
            }
        }
    }
}

keywords! {
    /// A `<timeline-range-name>` (Scroll-driven Animations §4.1).
    TimelineRangeName {
        Cover => "cover",
        Contain => "contain",
        Entry => "entry",
        Exit => "exit",
        EntryCrossing => "entry-crossing",
        ExitCrossing => "exit-crossing",
        Scroll => "scroll",
    }
}

/// One `animation-range-start` or `animation-range-end` entry: `normal`
/// (no name, no offset), a `<length-percentage>`, or a named range with an
/// optional offset (0% for a start, 100% for an end when omitted).
#[derive(Clone, Debug, PartialEq)]
pub struct RangeEdge {
    /// The named timeline range.
    pub name: Option<TimelineRangeName>,
    /// The offset within it (or within the whole timeline without a name).
    pub offset: Option<crate::LengthPercentage>,
}

impl RangeEdge {
    /// `normal`.
    pub const NORMAL: Self = Self {
        name: None,
        offset: None,
    };

    fn parse(parts: &[&Value]) -> Option<Self> {
        let (name, offset) = match parts {
            [one] if ident(one).is_some_and(|n| n.eq_ignore_ascii_case("normal")) => {
                return Some(Self::NORMAL);
            }
            [one] => match ident(one) {
                Some(n) => (Some(TimelineRangeName::from_name(n)?), None),
                None => (None, Some(crate::properties::length_percentage(one)?)),
            },
            [name, offset] => (
                Some(TimelineRangeName::from_name(ident(name)?)?),
                Some(crate::properties::length_percentage(offset)?),
            ),
            _ => return None,
        };
        Some(Self { name, offset })
    }
}

impl fmt::Display for RangeEdge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.name, &self.offset) {
            (None, None) => f.write_str("normal"),
            (Some(name), None) => f.write_str(name.name()),
            (None, Some(offset)) => write!(f, "{offset}"),
            (Some(name), Some(offset)) => write!(f, "{name} {offset}"),
        }
    }
}

/// One `view-timeline-inset` entry: start and end insets.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewInset(pub LengthPercentageOrAuto, pub LengthPercentageOrAuto);

impl fmt::Display for ViewInset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == self.1 {
            write!(f, "{}", self.0)
        } else {
            write!(f, "{} {}", self.0, self.1)
        }
    }
}

/// One `scroll-timeline-name` or `view-timeline-name` entry.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineName(pub Option<Arc<str>>);

impl fmt::Display for TimelineName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(name) => crate::serialize::serialize_identifier(f, name),
            None => f.write_str("none"),
        }
    }
}

/// The value of one transition or animation longhand (each is a list).
#[derive(Clone, Debug, PartialEq)]
pub enum AnimationValue {
    /// `transition-duration`, `transition-delay`, `animation-delay`.
    Times(Vec<Time>),
    /// `animation-duration`.
    Durations(Vec<AnimationDuration>),
    /// `transition-property`; empty for `none`.
    TransitionProperties(Vec<TransitionProperty>),
    /// `transition-behavior`.
    TransitionBehaviors(Vec<TransitionBehavior>),
    /// `animation-name`.
    Names(Vec<AnimationName>),
    /// `animation-iteration-count`.
    IterationCounts(Vec<IterationCount>),
    /// `animation-direction`.
    Directions(Vec<AnimationDirection>),
    /// `animation-fill-mode`.
    FillModes(Vec<FillMode>),
    /// `animation-play-state`.
    PlayStates(Vec<PlayState>),
    /// `animation-composition`.
    Compositions(Vec<CompositeOperation>),
    /// `animation-timeline`.
    Timelines(Vec<AnimationTimeline>),
    /// `scroll-timeline-name`, `view-timeline-name`.
    TimelineNames(Vec<TimelineName>),
    /// `scroll-timeline-axis`, `view-timeline-axis`.
    Axes(Vec<ScrollAxis>),
    /// `view-timeline-inset`.
    ViewInsets(Vec<ViewInset>),
    /// `animation-range-start`, `animation-range-end`.
    Ranges(Vec<RangeEdge>),
}

impl AnimationValue {
    /// The computed form: times in seconds.
    pub fn computed(&self) -> Self {
        match self {
            Self::Times(times) => Self::Times(times.iter().map(|t| t.computed()).collect()),
            Self::Durations(durations) => Self::Durations(
                durations
                    .iter()
                    .map(|d| match d {
                        AnimationDuration::Time(t) => AnimationDuration::Time(t.computed()),
                        AnimationDuration::Auto => AnimationDuration::Auto,
                    })
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// The number of entries (`transition-property: none` has none).
    pub fn len(&self) -> usize {
        match self {
            Self::Times(v) => v.len(),
            Self::Durations(v) => v.len(),
            Self::TransitionProperties(v) => v.len(),
            Self::TransitionBehaviors(v) => v.len(),
            Self::Names(v) => v.len(),
            Self::IterationCounts(v) => v.len(),
            Self::Directions(v) => v.len(),
            Self::FillModes(v) => v.len(),
            Self::PlayStates(v) => v.len(),
            Self::Compositions(v) => v.len(),
            Self::Timelines(v) => v.len(),
            Self::TimelineNames(v) => v.len(),
            Self::Axes(v) => v.len(),
            Self::ViewInsets(v) => v.len(),
            Self::Ranges(v) => v.len(),
        }
    }

    /// Whether the list is empty (only `transition-property: none`).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl fmt::Display for AnimationValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Times(v) => write_list(f, v),
            Self::Durations(v) => write_list(f, v),
            Self::TransitionProperties(v) if v.is_empty() => f.write_str("none"),
            Self::TransitionProperties(v) => write_list(f, v),
            Self::TransitionBehaviors(v) => write_list(f, v),
            Self::Names(v) => write_list(f, v),
            Self::IterationCounts(v) => write_list(f, v),
            Self::Directions(v) => write_list(f, v),
            Self::FillModes(v) => write_list(f, v),
            Self::PlayStates(v) => write_list(f, v),
            Self::Compositions(v) => write_list(f, v),
            Self::Timelines(v) => write_list(f, v),
            Self::TimelineNames(v) => write_list(f, v),
            Self::Axes(v) => write_list(f, v),
            Self::ViewInsets(v) => write_list(f, v),
            Self::Ranges(v) => write_list(f, v),
        }
    }
}

/// Entry `index` of a list, repeating the list as CSS Animations §4.2
/// and CSS Transitions §2 require (the list is never empty here).
pub fn cycle<T>(list: &[T], index: usize) -> Option<&T> {
    if list.is_empty() {
        None
    } else {
        list.get(index % list.len())
    }
}

/// The initial value of the transition and animation longhands.
pub(crate) fn initial(property: PropertyId) -> Option<AnimationValue> {
    use PropertyId as P;
    Some(match property {
        P::TransitionProperty => {
            AnimationValue::TransitionProperties(vec![TransitionProperty::All])
        }
        P::TransitionDuration | P::TransitionDelay | P::AnimationDelay => {
            AnimationValue::Times(vec![Time::ZERO])
        }
        P::TransitionBehavior => {
            AnimationValue::TransitionBehaviors(vec![TransitionBehavior::Normal])
        }
        P::AnimationName => AnimationValue::Names(vec![AnimationName::None]),
        P::AnimationDuration => AnimationValue::Durations(vec![AnimationDuration::Auto]),
        P::AnimationIterationCount => {
            AnimationValue::IterationCounts(vec![IterationCount::Number(1.0)])
        }
        P::AnimationDirection => AnimationValue::Directions(vec![AnimationDirection::Normal]),
        P::AnimationFillMode => AnimationValue::FillModes(vec![FillMode::None]),
        P::AnimationPlayState => AnimationValue::PlayStates(vec![PlayState::Running]),
        P::AnimationComposition => AnimationValue::Compositions(vec![CompositeOperation::Replace]),
        P::AnimationTimeline => AnimationValue::Timelines(vec![AnimationTimeline::Auto]),
        P::ScrollTimelineName | P::ViewTimelineName => {
            AnimationValue::TimelineNames(vec![TimelineName(None)])
        }
        P::ScrollTimelineAxis | P::ViewTimelineAxis => {
            AnimationValue::Axes(vec![ScrollAxis::Block])
        }
        P::ViewTimelineInset => AnimationValue::ViewInsets(vec![ViewInset(
            LengthPercentageOrAuto::Auto,
            LengthPercentageOrAuto::Auto,
        )]),
        P::AnimationRangeStart | P::AnimationRangeEnd => {
            AnimationValue::Ranges(vec![RangeEdge::NORMAL])
        }
        _ => return None,
    })
}

/// Split a value at top-level commas; `None` for an empty entry.
fn entries<'a>(parts: &[&'a Value]) -> Option<Vec<Vec<&'a Value>>> {
    let mut out = vec![Vec::new()];
    for part in parts {
        if part.token() == Some(&Token::Comma) {
            out.push(Vec::new());
        } else {
            out.last_mut().expect("nonempty").push(*part);
        }
    }
    out.iter().all(|e| !e.is_empty()).then_some(out)
}

fn each<T>(parts: &[&Value], entry: impl Fn(&Value) -> Option<T>) -> Option<Vec<T>> {
    entries(parts)?
        .into_iter()
        .map(|e| match e.as_slice() {
            [one] => entry(one),
            _ => None,
        })
        .collect()
}

fn nonnegative_time(value: &Value) -> Option<Time> {
    Time::parse(value).filter(|t| t.seconds >= 0.0)
}

fn transition_property(value: &Value) -> Option<TransitionProperty> {
    let name = ident(value)?;
    if name.starts_with("--") {
        return Some(TransitionProperty::Name(name.into()));
    }
    let lower = name.to_ascii_lowercase();
    if CssWideKeyword::from_name(&lower).is_some() || lower == "none" || lower == "default" {
        return None;
    }
    if lower == "all" {
        return Some(TransitionProperty::All);
    }
    Some(TransitionProperty::Name(lower.into()))
}

fn keyframes_name(value: &Value) -> Option<AnimationName> {
    match value.token()? {
        Token::String(s) => Some(AnimationName::Name {
            name: s.as_str().into(),
            quoted: true,
        }),
        Token::Ident(name) => {
            let lower = name.to_ascii_lowercase();
            if lower == "none" {
                return Some(AnimationName::None);
            }
            if CssWideKeyword::from_name(&lower).is_some() || lower == "default" {
                return None;
            }
            Some(AnimationName::Name {
                name: name.as_str().into(),
                quoted: false,
            })
        }
        _ => None,
    }
}

fn iteration_count(value: &Value) -> Option<IterationCount> {
    if ident(value).is_some_and(|n| n.eq_ignore_ascii_case("infinite")) {
        return Some(IterationCount::Infinite);
    }
    number(value)
        .filter(|n| *n >= 0.0)
        .map(IterationCount::Number)
}

fn timeline(value: &Value) -> Option<AnimationTimeline> {
    if let Some(name) = ident(value) {
        let lower = name.to_ascii_lowercase();
        return match lower.as_str() {
            "auto" => Some(AnimationTimeline::Auto),
            "none" => Some(AnimationTimeline::None),
            _ if name.starts_with("--") => Some(AnimationTimeline::Named(name.into())),
            _ => None,
        };
    }
    let ComponentValue::Function { name, arguments } = &value.value else {
        return None;
    };
    let args: Vec<&Value> = arguments.iter().filter(|v| !v.is_whitespace()).collect();
    match name.to_ascii_lowercase().as_str() {
        "scroll" => {
            let (mut scroller, mut axis) = (None, None);
            for arg in args {
                let name = ident(arg)?;
                if let (None, Some(s)) = (scroller, Scroller::from_name(name)) {
                    scroller = Some(s);
                } else if let (None, Some(a)) = (axis, ScrollAxis::from_name(name)) {
                    axis = Some(a);
                } else {
                    return None;
                }
            }
            Some(AnimationTimeline::Scroll(
                scroller.unwrap_or(Scroller::Nearest),
                axis.unwrap_or(ScrollAxis::Block),
            ))
        }
        "view" => {
            let (mut axis, mut insets) = (None, Vec::new());
            for arg in args {
                if let Some(a) = ident(arg).and_then(ScrollAxis::from_name) {
                    if axis.is_some() || !insets.is_empty() {
                        return None;
                    }
                    axis = Some(a);
                    continue;
                }
                let inset = if ident(arg).is_some_and(|n| n.eq_ignore_ascii_case("auto")) {
                    LengthPercentageOrAuto::Auto
                } else {
                    LengthPercentageOrAuto::LengthPercentage(crate::properties::length_percentage(
                        arg,
                    )?)
                };
                if insets.len() == 2 {
                    return None;
                }
                insets.push(inset);
            }
            let start = insets
                .first()
                .cloned()
                .unwrap_or(LengthPercentageOrAuto::Auto);
            let end = insets.get(1).cloned().unwrap_or_else(|| start.clone());
            Some(AnimationTimeline::View(
                axis.unwrap_or(ScrollAxis::Block),
                start,
                end,
            ))
        }
        _ => None,
    }
}

fn timeline_name(value: &Value) -> Option<TimelineName> {
    let name = ident(value)?;
    if name.eq_ignore_ascii_case("none") {
        Some(TimelineName(None))
    } else if name.starts_with("--") && name.len() > 2 {
        Some(TimelineName(Some(name.into())))
    } else {
        None
    }
}

fn inset(value: &Value) -> Option<LengthPercentageOrAuto> {
    if ident(value).is_some_and(|n| n.eq_ignore_ascii_case("auto")) {
        Some(LengthPercentageOrAuto::Auto)
    } else {
        crate::properties::length_percentage(value).map(LengthPercentageOrAuto::LengthPercentage)
    }
}

fn view_inset(parts: &[&Value]) -> Option<ViewInset> {
    match parts {
        [one] => {
            let one = inset(one)?;
            Some(ViewInset(one.clone(), one))
        }
        [start, end] => Some(ViewInset(inset(start)?, inset(end)?)),
        _ => None,
    }
}

/// `scroll-timeline` (`view` false) or `view-timeline`:
/// `[<name> <axis>?]#` (`view-timeline` also takes an inset).
pub(crate) fn timeline_shorthand(parts: &[&Value], view: bool) -> Option<Vec<PropertyValue>> {
    let mut names = Vec::new();
    let mut axes = Vec::new();
    let mut insets = Vec::new();
    for entry in entries(parts)? {
        let (first, rest) = entry.split_first()?;
        names.push(timeline_name(first)?);
        let mut axis = None;
        let mut inset_parts = Vec::new();
        for value in rest {
            match ident(value).and_then(ScrollAxis::from_name) {
                Some(a) if axis.is_none() && (inset_parts.is_empty() || view) => axis = Some(a),
                _ if view => inset_parts.push(*value),
                _ => return None,
            }
        }
        axes.push(axis.unwrap_or(ScrollAxis::Block));
        insets.push(if inset_parts.is_empty() {
            ViewInset(LengthPercentageOrAuto::Auto, LengthPercentageOrAuto::Auto)
        } else {
            view_inset(&inset_parts)?
        });
    }
    let mut out = vec![
        PropertyValue::Animation(AnimationValue::TimelineNames(names)),
        PropertyValue::Animation(AnimationValue::Axes(axes)),
    ];
    if view {
        out.push(PropertyValue::Animation(AnimationValue::ViewInsets(insets)));
    }
    Some(out)
}

/// `animation-range: [<animation-range-start> <animation-range-end>?]#`.
/// An omitted end is the start's named range at 100%, or `normal`.
pub(crate) fn range_shorthand(parts: &[&Value]) -> Option<Vec<PropertyValue>> {
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    for entry in entries(parts)? {
        // Try every split; the start takes as few values as it can.
        let mut found = None;
        for split in 1..=entry.len() {
            let (start, end) = entry.split_at(split);
            let Some(start) = RangeEdge::parse(start) else {
                continue;
            };
            if end.is_empty() {
                let end = match start.name {
                    Some(name) if start.offset.is_none() => RangeEdge {
                        name: Some(name),
                        offset: None,
                    },
                    _ => RangeEdge::NORMAL,
                };
                found = Some((start, end));
                break;
            }
            if let Some(end) = RangeEdge::parse(end) {
                found = Some((start, end));
                break;
            }
        }
        let (start, end) = found?;
        starts.push(start);
        ends.push(end);
    }
    Some(vec![
        PropertyValue::Animation(AnimationValue::Ranges(starts)),
        PropertyValue::Animation(AnimationValue::Ranges(ends)),
    ])
}

/// Parse one of the transition or animation longhands.
pub(crate) fn longhand(property: PropertyId, parts: &[&Value]) -> Option<PropertyValue> {
    use PropertyId as P;
    let value = match property {
        P::TransitionProperty => {
            if let [only] = parts {
                if ident(only).is_some_and(|n| n.eq_ignore_ascii_case("none")) {
                    return Some(PropertyValue::Animation(
                        AnimationValue::TransitionProperties(Vec::new()),
                    ));
                }
            }
            AnimationValue::TransitionProperties(each(parts, transition_property)?)
        }
        P::TransitionDuration => AnimationValue::Times(each(parts, nonnegative_time)?),
        P::TransitionDelay | P::AnimationDelay => AnimationValue::Times(each(parts, Time::parse)?),
        P::TransitionBehavior => AnimationValue::TransitionBehaviors(each(parts, |v| {
            TransitionBehavior::from_name(ident(v)?)
        })?),
        P::AnimationName => AnimationValue::Names(each(parts, keyframes_name)?),
        P::AnimationDuration => AnimationValue::Durations(each(parts, |v| {
            if ident(v).is_some_and(|n| n.eq_ignore_ascii_case("auto")) {
                Some(AnimationDuration::Auto)
            } else {
                nonnegative_time(v).map(AnimationDuration::Time)
            }
        })?),
        P::AnimationIterationCount => {
            AnimationValue::IterationCounts(each(parts, iteration_count)?)
        }
        P::AnimationDirection => {
            AnimationValue::Directions(each(parts, |v| AnimationDirection::from_name(ident(v)?))?)
        }
        P::AnimationFillMode => {
            AnimationValue::FillModes(each(parts, |v| FillMode::from_name(ident(v)?))?)
        }
        P::AnimationPlayState => {
            AnimationValue::PlayStates(each(parts, |v| PlayState::from_name(ident(v)?))?)
        }
        P::AnimationComposition => {
            AnimationValue::Compositions(each(parts, |v| CompositeOperation::from_name(ident(v)?))?)
        }
        P::AnimationTimeline => AnimationValue::Timelines(each(parts, timeline)?),
        P::ScrollTimelineName | P::ViewTimelineName => {
            AnimationValue::TimelineNames(each(parts, timeline_name)?)
        }
        P::ScrollTimelineAxis | P::ViewTimelineAxis => {
            AnimationValue::Axes(each(parts, |v| ScrollAxis::from_name(ident(v)?))?)
        }
        P::ViewTimelineInset => AnimationValue::ViewInsets(
            entries(parts)?
                .iter()
                .map(|e| view_inset(e))
                .collect::<Option<_>>()?,
        ),
        P::AnimationRangeStart | P::AnimationRangeEnd => AnimationValue::Ranges(
            entries(parts)?
                .iter()
                .map(|e| RangeEdge::parse(e))
                .collect::<Option<_>>()?,
        ),
        _ => return None,
    };
    Some(PropertyValue::Animation(value))
}

/// The longhands the `transition` shorthand sets, in its value order.
pub(crate) const TRANSITION: &[PropertyId] = &[
    PropertyId::TransitionProperty,
    PropertyId::TransitionDuration,
    PropertyId::TransitionTimingFunction,
    PropertyId::TransitionDelay,
    PropertyId::TransitionBehavior,
];

/// The longhands the `animation` shorthand sets (it also resets
/// `animation-composition`, `animation-timeline` and `animation-range`).
pub(crate) const ANIMATION: &[PropertyId] = &[
    PropertyId::AnimationName,
    PropertyId::AnimationDuration,
    PropertyId::AnimationTimingFunction,
    PropertyId::AnimationDelay,
    PropertyId::AnimationIterationCount,
    PropertyId::AnimationDirection,
    PropertyId::AnimationFillMode,
    PropertyId::AnimationPlayState,
    PropertyId::AnimationComposition,
    PropertyId::AnimationTimeline,
    PropertyId::AnimationRangeStart,
    PropertyId::AnimationRangeEnd,
];

/// `transition: <single-transition>#`.
pub(crate) fn transition_shorthand(parts: &[&Value]) -> Option<Vec<PropertyValue>> {
    let list = entries(parts)?;
    let count = list.len();
    let mut properties = Vec::new();
    let mut durations = Vec::new();
    let mut easings = Vec::new();
    let mut delays = Vec::new();
    let mut behaviors = Vec::new();
    for entry in &list {
        let (mut property, mut duration, mut delay, mut easing, mut behavior) =
            (None, None, None, None, None);
        for value in entry {
            if let Some(time) = Time::parse(value) {
                if duration.is_none() {
                    if time.seconds < 0.0 {
                        return None;
                    }
                    duration = Some(time);
                } else if delay.is_none() {
                    delay = Some(time);
                } else {
                    return None;
                }
            } else if let (None, Some(f)) = (&easing, EasingFunction::parse(value)) {
                easing = Some(f);
            } else if let (None, Some(b)) = (
                &behavior,
                ident(value).and_then(TransitionBehavior::from_name),
            ) {
                behavior = Some(b);
            } else if property.is_none() {
                if ident(value).is_some_and(|n| n.eq_ignore_ascii_case("none")) {
                    // `none` is valid only as the sole entry's property.
                    if count != 1 {
                        return None;
                    }
                    property = Some(None);
                } else {
                    property = Some(Some(transition_property(value)?));
                }
            } else {
                return None;
            }
        }
        if let Some(Some(p)) = property {
            properties.push(p);
        } else if property.is_none() {
            properties.push(TransitionProperty::All);
        }
        durations.push(duration.unwrap_or(Time::ZERO));
        delays.push(delay.unwrap_or(Time::ZERO));
        easings.push(easing.unwrap_or(EasingFunction::EASE));
        behaviors.push(behavior.unwrap_or(TransitionBehavior::Normal));
    }
    Some(vec![
        PropertyValue::Animation(AnimationValue::TransitionProperties(properties)),
        PropertyValue::Animation(AnimationValue::Times(durations)),
        PropertyValue::Easing(EasingList::from_functions(easings)),
        PropertyValue::Animation(AnimationValue::Times(delays)),
        PropertyValue::Animation(AnimationValue::TransitionBehaviors(behaviors)),
    ])
}

/// `animation: <single-animation>#`.
pub(crate) fn animation_shorthand(parts: &[&Value]) -> Option<Vec<PropertyValue>> {
    let list = entries(parts)?;
    let mut names = Vec::new();
    let mut durations = Vec::new();
    let mut easings = Vec::new();
    let mut delays = Vec::new();
    let mut counts = Vec::new();
    let mut directions = Vec::new();
    let mut fills = Vec::new();
    let mut states = Vec::new();
    for entry in &list {
        let (mut name, mut duration, mut delay, mut easing) = (None, None, None, None);
        let (mut count, mut direction, mut fill, mut state) = (None, None, None, None);
        for value in entry {
            let keyword = ident(value);
            if let Some(time) = Time::parse(value) {
                if duration.is_none() {
                    if time.seconds < 0.0 {
                        return None;
                    }
                    duration = Some(AnimationDuration::Time(time));
                } else if delay.is_none() {
                    delay = Some(time);
                } else {
                    return None;
                }
            } else if let (None, Some(f)) = (&easing, EasingFunction::parse(value)) {
                easing = Some(f);
            } else if let (None, Some(c)) = (&count, iteration_count(value)) {
                count = Some(c);
            } else if let (None, Some(d)) =
                (&direction, keyword.and_then(AnimationDirection::from_name))
            {
                direction = Some(d);
            } else if let (None, Some(m)) = (&fill, keyword.and_then(FillMode::from_name)) {
                // A keyword valid for another longhand goes there first,
                // so `none` is the fill mode before it is the name.
                fill = Some(m);
            } else if let (None, Some(s)) = (&state, keyword.and_then(PlayState::from_name)) {
                state = Some(s);
            } else if name.is_none() {
                name = Some(keyframes_name(value)?);
            } else {
                return None;
            }
        }
        names.push(name.unwrap_or(AnimationName::None));
        durations.push(duration.unwrap_or(AnimationDuration::Auto));
        easings.push(easing.unwrap_or(EasingFunction::EASE));
        delays.push(delay.unwrap_or(Time::ZERO));
        counts.push(count.unwrap_or(IterationCount::Number(1.0)));
        directions.push(direction.unwrap_or(AnimationDirection::Normal));
        fills.push(fill.unwrap_or(FillMode::None));
        states.push(state.unwrap_or(PlayState::Running));
    }
    Some(vec![
        PropertyValue::Animation(AnimationValue::Names(names)),
        PropertyValue::Animation(AnimationValue::Durations(durations)),
        PropertyValue::Easing(EasingList::from_functions(easings)),
        PropertyValue::Animation(AnimationValue::Times(delays)),
        PropertyValue::Animation(AnimationValue::IterationCounts(counts)),
        PropertyValue::Animation(AnimationValue::Directions(directions)),
        PropertyValue::Animation(AnimationValue::FillModes(fills)),
        PropertyValue::Animation(AnimationValue::PlayStates(states)),
        PropertyValue::Animation(AnimationValue::Compositions(vec![
            CompositeOperation::Replace,
        ])),
        PropertyValue::Animation(AnimationValue::Timelines(vec![AnimationTimeline::Auto])),
        PropertyValue::Animation(AnimationValue::Ranges(vec![RangeEdge::NORMAL])),
        PropertyValue::Animation(AnimationValue::Ranges(vec![RangeEdge::NORMAL])),
    ])
}
