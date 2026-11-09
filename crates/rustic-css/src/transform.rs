//! Transform values (CSS Transforms 1 and 2): `transform` functions, the
//! individual `translate`, `rotate` and `scale` properties, origins and
//! perspective.

use std::fmt;

use crate::calc::is_math_function;
use crate::color::write_number;
use crate::properties::{ident, keywords, length_percentage};
use crate::{Calc, ComponentValue, LengthPercentage, Token, Value};

/// One transform function, normalized: every 2D form is stored as its 3D
/// generalization. Angles are in degrees.
#[derive(Clone, Debug, PartialEq)]
pub enum TransformFunction {
    /// `matrix()` (`a b c d e f`, column-major as in CSS).
    Matrix([f64; 6]),
    /// `matrix3d()`, column-major.
    Matrix3d([f64; 16]),
    /// `translate*()`: x and y may be percentages of the reference box; z is
    /// a length.
    Translate(LengthPercentage, LengthPercentage, LengthPercentage),
    /// `scale*()`.
    Scale(f64, f64, f64),
    /// `rotate*()`: a rotation by `angle` degrees about the axis
    /// `(x, y, z)`, clockwise when looking down the axis.
    Rotate {
        /// Axis x.
        x: f64,
        /// Axis y.
        y: f64,
        /// Axis z.
        z: f64,
        /// Angle in degrees.
        angle: f64,
    },
    /// `skew*()`: x and y angles in degrees.
    Skew(f64, f64),
    /// `perspective()`: a distance, or `None` for `none` (no perspective).
    Perspective(Option<LengthPercentage>),
}

impl TransformFunction {
    /// The same function with `f` applied to every length.
    pub fn map_lengths(&self, f: &dyn Fn(&LengthPercentage) -> LengthPercentage) -> Self {
        match self {
            Self::Translate(x, y, z) => Self::Translate(f(x), f(y), f(z)),
            Self::Perspective(Some(d)) => Self::Perspective(Some(f(d))),
            other => other.clone(),
        }
    }
}

fn numbers(f: &mut fmt::Formatter<'_>, values: &[f64]) -> fmt::Result {
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write_number(f, *v)?;
    }
    Ok(())
}

fn degrees(f: &mut fmt::Formatter<'_>, value: f64) -> fmt::Result {
    write_number(f, value)?;
    f.write_str("deg")
}

impl fmt::Display for TransformFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let zero = LengthPercentage::ZERO;
        match self {
            Self::Matrix(m) => {
                f.write_str("matrix(")?;
                numbers(f, m)?;
                f.write_str(")")
            }
            Self::Matrix3d(m) => {
                f.write_str("matrix3d(")?;
                numbers(f, m)?;
                f.write_str(")")
            }
            Self::Translate(x, y, z) if *z == zero => write!(f, "translate({x}, {y})"),
            Self::Translate(x, y, z) => write!(f, "translate3d({x}, {y}, {z})"),
            Self::Scale(x, y, z) => {
                f.write_str(if *z == 1.0 { "scale(" } else { "scale3d(" })?;
                let values = [*x, *y, *z];
                numbers(f, if *z == 1.0 { &values[..2] } else { &values })?;
                f.write_str(")")
            }
            Self::Rotate { x, y, z, angle } if (*x, *y) == (0.0, 0.0) && *z > 0.0 => {
                f.write_str("rotate(")?;
                degrees(f, *angle)?;
                f.write_str(")")
            }
            Self::Rotate { x, y, z, angle } => {
                f.write_str("rotate3d(")?;
                numbers(f, &[*x, *y, *z])?;
                f.write_str(", ")?;
                degrees(f, *angle)?;
                f.write_str(")")
            }
            Self::Skew(x, y) => {
                f.write_str("skew(")?;
                degrees(f, *x)?;
                f.write_str(", ")?;
                degrees(f, *y)?;
                f.write_str(")")
            }
            Self::Perspective(None) => f.write_str("perspective(none)"),
            Self::Perspective(Some(d)) => write!(f, "perspective({d})"),
        }
    }
}

/// `transform`: functions in order; empty for `none`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransformList(pub Vec<TransformFunction>);

impl fmt::Display for TransformList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return f.write_str("none");
        }
        for (i, function) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{function}")?;
        }
        Ok(())
    }
}

/// `translate`, `rotate` or `scale` (CSS Transforms 2 §5): `None` for
/// `none`, else one [`TransformFunction`] of the matching kind.
#[derive(Clone, Debug, PartialEq)]
pub struct IndividualTransform(pub Option<TransformFunction>);

impl fmt::Display for IndividualTransform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let zero = LengthPercentage::ZERO;
        match &self.0 {
            None => f.write_str("none"),
            Some(TransformFunction::Translate(x, y, z)) if *z != zero => write!(f, "{x} {y} {z}"),
            Some(TransformFunction::Translate(x, y, _)) if *y != zero => write!(f, "{x} {y}"),
            Some(TransformFunction::Translate(x, _, _)) => write!(f, "{x}"),
            Some(TransformFunction::Scale(x, y, z)) => {
                write_number(f, *x)?;
                if *y != *x || *z != 1.0 {
                    f.write_str(" ")?;
                    write_number(f, *y)?;
                }
                if *z != 1.0 {
                    f.write_str(" ")?;
                    write_number(f, *z)?;
                }
                Ok(())
            }
            Some(TransformFunction::Rotate { x, y, z, angle }) => {
                if (*x, *y) != (0.0, 0.0) || *z <= 0.0 {
                    write_number(f, *x)?;
                    f.write_str(" ")?;
                    write_number(f, *y)?;
                    f.write_str(" ")?;
                    write_number(f, *z)?;
                    f.write_str(" ")?;
                }
                degrees(f, *angle)
            }
            Some(other) => write!(f, "{other}"),
        }
    }
}

keywords! {
    /// `backface-visibility`.
    BackfaceVisibility {
        Visible => "visible",
        Hidden => "hidden",
    }
}

keywords! {
    /// `transform-style`.
    TransformStyle {
        Flat => "flat",
        Preserve3d => "preserve-3d",
    }
}

/// An `<angle>` in degrees; a unitless zero is allowed (CSS Transforms 1
/// §6.1 keeps it for compatibility).
fn angle(value: &Value) -> Option<f64> {
    let degrees = match value.token()? {
        Token::Dimension(n, unit) => {
            n.value
                * match unit.to_ascii_lowercase().as_str() {
                    "deg" => 1.0,
                    "grad" => 0.9,
                    "rad" => 180.0 / std::f64::consts::PI,
                    "turn" => 360.0,
                    _ => return None,
                }
        }
        Token::Number(n) if n.value == 0.0 => 0.0,
        _ => return None,
    };
    degrees.is_finite().then_some(degrees)
}

/// A `<number>`, or a math function resolving to one.
fn number(value: &Value) -> Option<f64> {
    if matches!(&value.value, ComponentValue::Function { name, .. } if is_math_function(name)) {
        return Calc::parse_number(value);
    }
    match value.token()? {
        Token::Number(n) => Some(n.value),
        _ => None,
    }
}

/// A `<number>` or `<percentage>` (a scale factor).
fn factor(value: &Value) -> Option<f64> {
    match value.token() {
        Some(Token::Percentage(p)) => Some(p.value / 100.0),
        _ => number(value),
    }
}

/// A `<length>` (no percentage), as a length-percentage.
fn length(value: &Value) -> Option<LengthPercentage> {
    length_percentage(value)
        .filter(|lp| !matches!(lp, LengthPercentage::Percentage(_)))
        .filter(
            |lp| !matches!(lp, LengthPercentage::Calc(c) if c.kind() != crate::CalcKind::Length),
        )
}

fn function(value: &Value) -> Option<TransformFunction> {
    use TransformFunction as T;
    let ComponentValue::Function { name, arguments } = &value.value else {
        return None;
    };
    let args: Vec<&Value> = arguments.iter().filter(|v| !v.is_whitespace()).collect();
    let groups: Vec<&[&Value]> = args.split(|v| v.token() == Some(&Token::Comma)).collect();
    let mut single = Vec::with_capacity(groups.len());
    for group in &groups {
        let [only] = group else {
            return None;
        };
        single.push(*only);
    }
    let zero = LengthPercentage::ZERO;
    let all = |f: fn(&Value) -> Option<f64>| -> Option<Vec<f64>> {
        single.iter().map(|v| f(v)).collect()
    };
    Some(
        match (name.to_ascii_lowercase().as_str(), single.as_slice()) {
            ("matrix", _) => T::Matrix(all(number)?.try_into().ok()?),
            ("matrix3d", _) => T::Matrix3d(all(number)?.try_into().ok()?),
            ("translate", [x]) => T::Translate(length_percentage(x)?, zero.clone(), zero),
            ("translate", [x, y]) => {
                T::Translate(length_percentage(x)?, length_percentage(y)?, zero)
            }
            ("translatex", [x]) => T::Translate(length_percentage(x)?, zero.clone(), zero),
            ("translatey", [y]) => T::Translate(zero.clone(), length_percentage(y)?, zero),
            ("translatez", [z]) => T::Translate(zero.clone(), zero, length(z)?),
            ("translate3d", [x, y, z]) => {
                T::Translate(length_percentage(x)?, length_percentage(y)?, length(z)?)
            }
            ("scale", [s]) => {
                let s = factor(s)?;
                T::Scale(s, s, 1.0)
            }
            ("scale", [x, y]) => T::Scale(factor(x)?, factor(y)?, 1.0),
            ("scalex", [x]) => T::Scale(factor(x)?, 1.0, 1.0),
            ("scaley", [y]) => T::Scale(1.0, factor(y)?, 1.0),
            ("scalez", [z]) => T::Scale(1.0, 1.0, factor(z)?),
            ("scale3d", [x, y, z]) => T::Scale(factor(x)?, factor(y)?, factor(z)?),
            ("rotate" | "rotatez", [a]) => T::Rotate {
                x: 0.0,
                y: 0.0,
                z: 1.0,
                angle: angle(a)?,
            },
            ("rotatex", [a]) => T::Rotate {
                x: 1.0,
                y: 0.0,
                z: 0.0,
                angle: angle(a)?,
            },
            ("rotatey", [a]) => T::Rotate {
                x: 0.0,
                y: 1.0,
                z: 0.0,
                angle: angle(a)?,
            },
            ("rotate3d", [x, y, z, a]) => T::Rotate {
                x: number(x)?,
                y: number(y)?,
                z: number(z)?,
                angle: angle(a)?,
            },
            ("skew", [x]) => T::Skew(angle(x)?, 0.0),
            ("skew", [x, y]) => T::Skew(angle(x)?, angle(y)?),
            ("skewx", [x]) => T::Skew(angle(x)?, 0.0),
            ("skewy", [y]) => T::Skew(0.0, angle(y)?),
            ("perspective", [d]) => {
                if ident(d).is_some_and(|i| i.eq_ignore_ascii_case("none")) {
                    T::Perspective(None)
                } else {
                    T::Perspective(Some(length(d).filter(|d| !d.is_negative())?))
                }
            }
            _ => return None,
        },
    )
}

/// Parse `transform`: `none` or a whitespace-separated function list.
pub(crate) fn transform_list(parts: &[&Value]) -> Option<TransformList> {
    if let [only] = parts {
        if ident(only).is_some_and(|i| i.eq_ignore_ascii_case("none")) {
            return Some(TransformList::default());
        }
    }
    if parts.is_empty() {
        return None;
    }
    parts
        .iter()
        .map(|v| function(v))
        .collect::<Option<_>>()
        .map(TransformList)
}

fn is_none(parts: &[&Value]) -> bool {
    matches!(parts, [only] if ident(only).is_some_and(|i| i.eq_ignore_ascii_case("none")))
}

/// Parse `translate`: `none | <length-percentage> [<length-percentage>
/// <length>?]?`.
pub(crate) fn translate(parts: &[&Value]) -> Option<IndividualTransform> {
    if is_none(parts) {
        return Some(IndividualTransform(None));
    }
    let zero = LengthPercentage::ZERO;
    let function = match parts {
        [x] => TransformFunction::Translate(length_percentage(x)?, zero.clone(), zero),
        [x, y] => TransformFunction::Translate(length_percentage(x)?, length_percentage(y)?, zero),
        [x, y, z] => {
            TransformFunction::Translate(length_percentage(x)?, length_percentage(y)?, length(z)?)
        }
        _ => return None,
    };
    Some(IndividualTransform(Some(function)))
}

/// Parse `scale`: `none | [<number> | <percentage>]{1,3}`.
pub(crate) fn scale(parts: &[&Value]) -> Option<IndividualTransform> {
    if is_none(parts) {
        return Some(IndividualTransform(None));
    }
    let (x, y, z) = match parts {
        [s] => {
            let s = factor(s)?;
            (s, s, 1.0)
        }
        [x, y] => (factor(x)?, factor(y)?, 1.0),
        [x, y, z] => (factor(x)?, factor(y)?, factor(z)?),
        _ => return None,
    };
    Some(IndividualTransform(Some(TransformFunction::Scale(x, y, z))))
}

/// Parse `rotate`: `none | <angle> | [x | y | z | <number>{3}] && <angle>`.
pub(crate) fn rotate(parts: &[&Value]) -> Option<IndividualTransform> {
    if is_none(parts) {
        return Some(IndividualTransform(None));
    }
    let axis_name = |v: &Value| match ident(v).map(str::to_ascii_lowercase).as_deref() {
        Some("x") => Some((1.0, 0.0, 0.0)),
        Some("y") => Some((0.0, 1.0, 0.0)),
        Some("z") => Some((0.0, 0.0, 1.0)),
        _ => None,
    };
    let ((x, y, z), a) = match parts {
        [a] => ((0.0, 0.0, 1.0), angle(a)?),
        [p, q] => match (axis_name(p), axis_name(q)) {
            (Some(axis), None) => (axis, angle(q)?),
            (None, Some(axis)) => (axis, angle(p)?),
            _ => return None,
        },
        [x, y, z, a] => match angle(a) {
            Some(a) => ((number(x)?, number(y)?, number(z)?), a),
            None => ((number(y)?, number(z)?, number(a)?), angle(x)?),
        },
        _ => return None,
    };
    Some(IndividualTransform(Some(TransformFunction::Rotate {
        x,
        y,
        z,
        angle: a,
    })))
}
