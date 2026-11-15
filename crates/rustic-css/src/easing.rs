//! CSS Easing Functions Level 2 §2.

use std::fmt;
use std::sync::Arc;

use crate::background::{list, write_list};
use crate::color::write_number;
use crate::properties::{integer, number_in};
use crate::{Calc, CalcKind, ComponentValue, Token, Value};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    input: f64,
    output: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepPosition {
    Start,
    End,
    JumpStart,
    JumpNone,
    JumpBoth,
}

#[derive(Clone, Debug, PartialEq)]
enum Easing {
    Linear,
    Piecewise(Arc<[Point]>),
    Bezier {
        points: [f64; 4],
        keyword: Option<&'static str>,
    },
    Steps(i32, StepPosition),
}

/// A parsed CSS timing function mapping input progress to output progress.
#[derive(Clone, Debug, PartialEq)]
pub struct EasingFunction(Easing);

impl EasingFunction {
    /// The initial CSS timing function.
    pub const EASE: Self = Self::bezier([0.25, 0.1, 0.25, 1.0], Some("ease"));

    const fn bezier(points: [f64; 4], keyword: Option<&'static str>) -> Self {
        Self(Easing::Bezier { points, keyword })
    }

    /// Parse one `<easing-function>` CSS component value.
    pub fn parse(value: &Value) -> Option<Self> {
        if let Some(Token::Ident(name)) = value.token() {
            return Some(match name.to_ascii_lowercase().as_str() {
                "linear" => Self(Easing::Linear),
                "ease" => Self::EASE,
                "ease-in" => Self::bezier([0.42, 0.0, 1.0, 1.0], Some("ease-in")),
                "ease-out" => Self::bezier([0.0, 0.0, 0.58, 1.0], Some("ease-out")),
                "ease-in-out" => Self::bezier([0.42, 0.0, 0.58, 1.0], Some("ease-in-out")),
                "step-start" => Self(Easing::Steps(1, StepPosition::Start)),
                "step-end" => Self(Easing::Steps(1, StepPosition::End)),
                _ => return None,
            });
        }
        let ComponentValue::Function { name, arguments } = &value.value else {
            return None;
        };
        let parts: Vec<&Value> = arguments
            .iter()
            .filter(|value| !value.is_whitespace())
            .collect();
        let kind = match name.to_ascii_lowercase().as_str() {
            "linear" => Easing::Piecewise(linear_points(&parts)?),
            "cubic-bezier" => {
                let [x1, comma1, y1, comma2, x2, comma3, y2] = parts.as_slice() else {
                    return None;
                };
                if ![comma1, comma2, comma3]
                    .into_iter()
                    .all(|comma| is_comma(comma))
                {
                    return None;
                }
                return Some(Self::bezier(
                    [
                        number_in(x1, 0.0, 1.0)?,
                        number(y1)?,
                        number_in(x2, 0.0, 1.0)?,
                        number(y2)?,
                    ],
                    None,
                ));
            }
            "steps" => parse_steps(&parts)?,
            _ => return None,
        };
        Some(Self(kind))
    }

    /// Sample at `input`, including extrapolation outside 0–1.
    ///
    /// For step functions `before` selects the lower value at a jump.
    pub fn sample(&self, input: f64, before: bool) -> f64 {
        if input.is_nan() {
            return input;
        }
        match &self.0 {
            Easing::Linear => input,
            Easing::Piecewise(points) => sample_linear(points, input),
            Easing::Bezier { points, .. } => sample_bezier(*points, input),
            Easing::Steps(count, position) => sample_steps(*count, *position, input, before),
        }
    }
}

/// The list-valued timing functions shared by the two CSS timing longhands.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EasingList(Option<Arc<[EasingFunction]>>);

impl EasingList {
    pub(crate) fn parse(values: &[&Value]) -> Option<Self> {
        let items = list(values, |entry| match entry {
            [one] => EasingFunction::parse(one),
            _ => None,
        })?;
        Some(Self(
            (items.as_slice() != [EasingFunction::EASE]).then(|| items.into()),
        ))
    }

    /// The timing functions in list order, defaulting to one `ease` entry.
    pub fn as_slice(&self) -> &[EasingFunction] {
        static INITIAL: [EasingFunction; 1] = [EasingFunction::EASE];
        self.0.as_deref().unwrap_or(&INITIAL)
    }
}

fn is_comma(value: &Value) -> bool {
    value.token() == Some(&Token::Comma)
}

fn number(value: &Value) -> Option<f64> {
    number_in(value, -f64::MAX, f64::MAX)
}

fn percentage(value: &Value) -> Option<f64> {
    if let Some(Token::Percentage(p)) = value.token() {
        return Some(p.value);
    }
    let calc = Calc::parse_length_percentage(value)?;
    (calc.kind() == CalcKind::Percentage).then_some(())?;
    calc.resolve(|_| 0.0, Some(100.0))
}

fn linear_stop(stop: &[&Value]) -> Option<(f64, Option<f64>, Option<f64>)> {
    match *stop {
        [a] => Some((number(a)?, None, None)),
        [a, b] => match number(a) {
            Some(output) => Some((output, Some(percentage(b)?), None)),
            None => Some((number(b)?, Some(percentage(a)?), None)),
        },
        [a, b, c] => match number(a) {
            Some(output) => Some((output, Some(percentage(b)?), Some(percentage(c)?))),
            None => Some((number(c)?, Some(percentage(a)?), Some(percentage(b)?))),
        },
        _ => None,
    }
}

fn linear_points(parts: &[&Value]) -> Option<Arc<[Point]>> {
    let stops = list(parts, linear_stop)?;
    if stops.len() < 2 {
        return None;
    }
    let last = stops.len() - 1;
    let mut points = Vec::with_capacity(stops.len());
    let mut largest = f64::NEG_INFINITY;
    let mut previous = 0;
    for (index, (output, mut input, extra)) in stops.into_iter().enumerate() {
        if index == 0 {
            input = Some(input.unwrap_or(0.0));
        } else if index == last {
            input = Some(input.unwrap_or(100.0));
        }
        for input in std::iter::once(input).chain(extra.map(Some)) {
            let next = points.len();
            points.push(Point { input: 0.0, output });
            let Some(input) = input else {
                continue;
            };
            let input = (input / 100.0).max(largest);
            points[next].input = input;
            let gap = u32::try_from(next - previous).ok()?;
            if gap > 1 {
                for (offset, point) in (1..gap).zip(&mut points[previous + 1..next]) {
                    let t = f64::from(offset) / f64::from(gap);
                    point.input = ((input - largest) * t + largest).min(input);
                }
            }
            previous = next;
            largest = input;
        }
    }
    Some(points.into())
}

fn parse_steps(parts: &[&Value]) -> Option<Easing> {
    let (value, position) = match *parts {
        [count] => (count, StepPosition::End),
        [count, comma, position] if is_comma(comma) => {
            let Token::Ident(name) = position.token()? else {
                return None;
            };
            let position = match name.to_ascii_lowercase().as_str() {
                "start" => StepPosition::Start,
                "end" | "jump-end" => StepPosition::End,
                "jump-start" => StepPosition::JumpStart,
                "jump-none" => StepPosition::JumpNone,
                "jump-both" => StepPosition::JumpBoth,
                _ => return None,
            };
            (count, position)
        }
        _ => return None,
    };
    let count = integer(value)?;
    let minimum = if position == StepPosition::JumpNone {
        2
    } else {
        1
    };
    if count < minimum && value.token().is_some() {
        return None;
    }
    Some(Easing::Steps(count.max(minimum), position))
}

fn sample_steps(count: i32, position: StepPosition, input: f64, before: bool) -> f64 {
    let count = f64::from(count);
    let jumps = match position {
        StepPosition::JumpNone => count - 1.0,
        StepPosition::JumpBoth => count + 1.0,
        _ => count,
    };
    let scaled = input * count;
    if !scaled.is_finite() {
        return input * (count / jumps);
    }
    let mut step = scaled.floor();
    if matches!(
        position,
        StepPosition::Start | StepPosition::JumpStart | StepPosition::JumpBoth
    ) {
        step += 1.0;
    }
    if before && scaled.fract() == 0.0 {
        step -= 1.0;
    }
    if input >= 0.0 {
        step = step.max(0.0);
    }
    if input <= 1.0 {
        step = step.min(jumps);
    }
    step / jumps
}

fn sample_linear(points: &[Point], input: f64) -> f64 {
    let left = points.partition_point(|point| point.input <= input);
    let index = left.saturating_sub(1).min(points.len() - 2);
    let (a, b) = (points[index], points[index + 1]);
    if a.input == b.input || a.output == b.output {
        return b.output;
    }
    let t = (input - a.input) / (b.input - a.input);
    interpolate(a.output, b.output, t)
}

fn interpolate(a: f64, b: f64, t: f64) -> f64 {
    if (0.0..=1.0).contains(&t) {
        a * (1.0 - t) + b * t
    } else {
        a + (b - a) * t
    }
}

fn sample_bezier([x1, y1, x2, y2]: [f64; 4], input: f64) -> f64 {
    let tangent = |x: f64, y: f64, from: f64| {
        if y == from {
            from
        } else {
            from + (y - from) * ((input - from) / (x - from))
        }
    };
    if input < 0.0 {
        return if x1 > 0.0 {
            tangent(x1, y1, 0.0)
        } else if x2 > 0.0 {
            tangent(x2, y2, 0.0)
        } else {
            0.0
        };
    }
    if input > 1.0 {
        return if x2 < 1.0 {
            tangent(x2, y2, 1.0)
        } else if x1 < 1.0 {
            tangent(x1, y1, 1.0)
        } else {
            1.0
        };
    }
    if input == 0.0 || input == 1.0 || (x1 == y1 && x2 == y2) {
        return input;
    }
    let component = |t: f64, a: f64, b: f64| {
        let u = 1.0 - t;
        (3.0 * t * u * u) * a + (3.0 * t * t * u) * b + t * t * t
    };
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..56 {
        let t = (lo + hi) / 2.0;
        let x = component(t, x1, x2);
        if x == input {
            return component(t, y1, y2);
        }
        if x < input {
            lo = t;
        } else {
            hi = t;
        }
    }
    component((lo + hi) / 2.0, y1, y2)
}

impl fmt::Display for EasingList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_list(f, self.as_slice())
    }
}

impl fmt::Display for EasingFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Easing::Linear => f.write_str("linear"),
            Easing::Bezier {
                keyword: Some(keyword),
                ..
            } => f.write_str(keyword),
            Easing::Bezier {
                points,
                keyword: None,
            } => {
                f.write_str("cubic-bezier(")?;
                for (index, &point) in points.iter().enumerate() {
                    if index != 0 {
                        f.write_str(", ")?;
                    }
                    write_number(f, point)?;
                }
                f.write_str(")")
            }
            Easing::Steps(count, position) => {
                let name = match position {
                    StepPosition::Start => "start",
                    StepPosition::JumpStart => "jump-start",
                    StepPosition::JumpNone => "jump-none",
                    StepPosition::JumpBoth => "jump-both",
                    StepPosition::End => return write!(f, "steps({count})"),
                };
                write!(f, "steps({count}, {name})")
            }
            Easing::Piecewise(points) => {
                f.write_str("linear(")?;
                for (index, point) in points.iter().enumerate() {
                    if index != 0 {
                        f.write_str(", ")?;
                    }
                    write_number(f, point.output)?;
                    f.write_str(" ")?;
                    write_number(f, crate::calc::censor(point.input * 100.0))?;
                    f.write_str("%")?;
                }
                f.write_str(")")
            }
        }
    }
}
