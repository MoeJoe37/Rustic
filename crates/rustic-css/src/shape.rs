//! `clip-path` and its `<basic-shape>` values (CSS Masking 1 §5, CSS Shapes
//! 1 §3).

use std::fmt;

use crate::background::{PositionComponent, PositionValue, position};
use crate::properties::{
    CornerRadius, PropertyValue, ident, keywords, length_percentage, nonnegative, shorthand,
};
use crate::{ComponentValue, LengthPercentage, Token, Value};

/// A circle or ellipse radius (`<shape-radius>`).
#[derive(Clone, Debug, PartialEq)]
pub enum ShapeRadius {
    /// A nonnegative length; percentages refer to the reference box (see
    /// CSS Shapes 1 §3.1).
    Length(LengthPercentage),
    /// The distance to the nearest side of the reference box.
    ClosestSide,
    /// The distance to the farthest side of the reference box.
    FarthestSide,
}

impl fmt::Display for ShapeRadius {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length(lp) => write!(f, "{lp}"),
            Self::ClosestSide => f.write_str("closest-side"),
            Self::FarthestSide => f.write_str("farthest-side"),
        }
    }
}

/// A `<basic-shape>` that `rustic-css` supports.
#[derive(Clone, Debug, PartialEq)]
pub enum BasicShape {
    /// `inset()`: offsets from the top, right, bottom and left edges of the
    /// reference box, with corner radii (top-left first, clockwise).
    Inset {
        /// Top, right, bottom and left offsets.
        offsets: [LengthPercentage; 4],
        /// Corner radii; percentages refer to the inset rectangle.
        radii: [CornerRadius; 4],
    },
    /// `circle()`.
    Circle {
        /// The radius.
        radius: ShapeRadius,
        /// The center; `None` is the center of the reference box.
        center: Option<PositionValue>,
    },
    /// `ellipse()`.
    Ellipse {
        /// The horizontal radius.
        rx: ShapeRadius,
        /// The vertical radius.
        ry: ShapeRadius,
        /// The center; `None` is the center of the reference box.
        center: Option<PositionValue>,
    },
    /// `polygon()`.
    Polygon {
        /// Whether the fill rule is `evenodd` (else `nonzero`).
        evenodd: bool,
        /// The vertices, at least one.
        points: Vec<(LengthPercentage, LengthPercentage)>,
    },
}

keywords! {
    /// The reference box of a `clip-path` (`<geometry-box>`).
    GeometryBox {
        MarginBox => "margin-box",
        BorderBox => "border-box",
        PaddingBox => "padding-box",
        ContentBox => "content-box",
        FillBox => "fill-box",
        StrokeBox => "stroke-box",
        ViewBox => "view-box",
    }
}

/// A `clip-path` value.
#[derive(Clone, Debug, PartialEq)]
pub enum ClipPath {
    /// `none`.
    None,
    /// A reference to an SVG `<clipPath>` element.
    Url(String),
    /// A basic shape in a reference box (`border-box` when omitted).
    Shape(BasicShape, Option<GeometryBox>),
    /// A reference box alone: clip to its (rounded) outline.
    Box(GeometryBox),
}

impl BasicShape {
    /// The same shape with `f` applied to every length, and every center
    /// position in its offset form (as computed values are).
    pub fn map_lengths(&self, f: &dyn Fn(&LengthPercentage) -> LengthPercentage) -> Self {
        let radius = |r: &ShapeRadius| match r {
            ShapeRadius::Length(lp) => ShapeRadius::Length(f(lp)),
            other => other.clone(),
        };
        let center = |c: &Option<PositionValue>| {
            c.as_ref().map(|p| PositionValue {
                x: PositionComponent::Offset(f(&p.x.to_offset())),
                y: PositionComponent::Offset(f(&p.y.to_offset())),
            })
        };
        match self {
            Self::Inset { offsets, radii } => Self::Inset {
                offsets: offsets.clone().map(|lp| f(&lp)),
                radii: radii.clone().map(|r| CornerRadius {
                    horizontal: f(&r.horizontal),
                    vertical: f(&r.vertical),
                }),
            },
            Self::Circle {
                radius: r,
                center: c,
            } => Self::Circle {
                radius: radius(r),
                center: center(c),
            },
            Self::Ellipse { rx, ry, center: c } => Self::Ellipse {
                rx: radius(rx),
                ry: radius(ry),
                center: center(c),
            },
            Self::Polygon { evenodd, points } => Self::Polygon {
                evenodd: *evenodd,
                points: points.iter().map(|(x, y)| (f(x), f(y))).collect(),
            },
        }
    }
}

impl fmt::Display for BasicShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = |f: &mut fmt::Formatter<'_>, c: &Option<PositionValue>| match c {
            Some(p) => write!(f, " at {p}"),
            None => Ok(()),
        };
        match self {
            Self::Inset { offsets, radii } => {
                let [t, r, b, l] = offsets;
                write!(f, "inset({t} {r} {b} {l}")?;
                if radii.iter().any(|c| *c != CornerRadius::ZERO) {
                    let [a, b, c, d] = radii;
                    write!(
                        f,
                        " round {} {} {} {} / {} {} {} {}",
                        a.horizontal,
                        b.horizontal,
                        c.horizontal,
                        d.horizontal,
                        a.vertical,
                        b.vertical,
                        c.vertical,
                        d.vertical
                    )?;
                }
                f.write_str(")")
            }
            Self::Circle { radius, center } => {
                write!(f, "circle({radius}")?;
                at(f, center)?;
                f.write_str(")")
            }
            Self::Ellipse { rx, ry, center } => {
                write!(f, "ellipse({rx} {ry}")?;
                at(f, center)?;
                f.write_str(")")
            }
            Self::Polygon { evenodd, points } => {
                f.write_str("polygon(")?;
                if *evenodd {
                    f.write_str("evenodd, ")?;
                }
                for (i, (x, y)) in points.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{x} {y}")?;
                }
                f.write_str(")")
            }
        }
    }
}

impl fmt::Display for ClipPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Url(url) => {
                f.write_str("url(")?;
                crate::serialize::serialize_string(f, url)?;
                f.write_str(")")
            }
            Self::Shape(shape, None) => write!(f, "{shape}"),
            Self::Shape(shape, Some(b)) => write!(f, "{shape} {b}"),
            Self::Box(b) => write!(f, "{b}"),
        }
    }
}

fn shape_radius(value: &Value) -> Option<ShapeRadius> {
    match ident(value).map(str::to_ascii_lowercase).as_deref() {
        Some("closest-side") => Some(ShapeRadius::ClosestSide),
        Some("farthest-side") => Some(ShapeRadius::FarthestSide),
        Some(_) => None,
        None => nonnegative(value).map(ShapeRadius::Length),
    }
}

/// Split `parts` at an `at` keyword: (before, position after it).
fn split_at<'a, 'b>(parts: &'b [&'a Value]) -> Option<(&'b [&'a Value], Option<PositionValue>)> {
    match parts
        .iter()
        .position(|v| ident(v).is_some_and(|i| i.eq_ignore_ascii_case("at")))
    {
        Some(i) => Some((&parts[..i], Some(position(&parts[i + 1..])?))),
        None => Some((parts, None)),
    }
}

/// Parse one `<basic-shape>` function.
fn basic_shape(value: &Value) -> Option<BasicShape> {
    let ComponentValue::Function { name, arguments } = &value.value else {
        return None;
    };
    let args: Vec<&Value> = arguments.iter().filter(|v| !v.is_whitespace()).collect();
    match name.to_ascii_lowercase().as_str() {
        "inset" => {
            let round = args
                .iter()
                .position(|v| ident(v).is_some_and(|i| i.eq_ignore_ascii_case("round")));
            let (lengths, radii) = match round {
                Some(i) => (&args[..i], Some(&args[i + 1..])),
                None => (&args[..], None),
            };
            let lengths: Vec<LengthPercentage> = lengths
                .iter()
                .map(|v| length_percentage(v))
                .collect::<Option<_>>()?;
            let [t, r, b, l] = match lengths.as_slice() {
                [a] => [a, a, a, a],
                [a, b] => [a, b, a, b],
                [a, b, c] => [a, b, c, b],
                [a, b, c, d] => [a, b, c, d],
                _ => return None,
            };
            let radii = match radii {
                None => [
                    CornerRadius::ZERO,
                    CornerRadius::ZERO,
                    CornerRadius::ZERO,
                    CornerRadius::ZERO,
                ],
                Some(parts) => {
                    let values = shorthand("border-radius", parts)?;
                    let mut out = [
                        CornerRadius::ZERO,
                        CornerRadius::ZERO,
                        CornerRadius::ZERO,
                        CornerRadius::ZERO,
                    ];
                    for (slot, value) in out.iter_mut().zip(values) {
                        let PropertyValue::BorderRadius(r) = value else {
                            return None;
                        };
                        *slot = r;
                    }
                    out
                }
            };
            Some(BasicShape::Inset {
                offsets: [t.clone(), r.clone(), b.clone(), l.clone()],
                radii,
            })
        }
        "circle" => {
            let (radius, center) = split_at(&args)?;
            let radius = match radius {
                [] => ShapeRadius::ClosestSide,
                [r] => shape_radius(r)?,
                _ => return None,
            };
            Some(BasicShape::Circle { radius, center })
        }
        "ellipse" => {
            let (radii, center) = split_at(&args)?;
            let (rx, ry) = match radii {
                [] => (ShapeRadius::ClosestSide, ShapeRadius::ClosestSide),
                [x, y] => (shape_radius(x)?, shape_radius(y)?),
                _ => return None,
            };
            Some(BasicShape::Ellipse { rx, ry, center })
        }
        "polygon" => {
            let mut groups = args.split(|v| v.token() == Some(&Token::Comma));
            let mut evenodd = false;
            let mut points = Vec::new();
            let mut first = true;
            for group in groups.by_ref() {
                if first {
                    first = false;
                    if let [rule] = group {
                        match ident(rule).map(str::to_ascii_lowercase).as_deref() {
                            Some("nonzero") => continue,
                            Some("evenodd") => {
                                evenodd = true;
                                continue;
                            }
                            _ => {}
                        }
                    }
                }
                let [x, y] = group else {
                    return None;
                };
                points.push((length_percentage(x)?, length_percentage(y)?));
            }
            (!points.is_empty()).then_some(BasicShape::Polygon { evenodd, points })
        }
        _ => None,
    }
}

/// Parse a `clip-path` value.
pub(crate) fn clip_path(parts: &[&Value]) -> Option<ClipPath> {
    if let [only] = parts {
        if ident(only).is_some_and(|i| i.eq_ignore_ascii_case("none")) {
            return Some(ClipPath::None);
        }
        if let Some(crate::background::Image::Url(url)) = crate::background::image(only) {
            return Some(ClipPath::Url(url));
        }
    }
    let (mut shape, mut reference) = (None, None);
    for part in parts {
        if shape.is_none() {
            if let Some(s) = basic_shape(part) {
                shape = Some(s);
                continue;
            }
        }
        if reference.is_none() {
            if let Some(b) = ident(part).and_then(GeometryBox::from_name) {
                reference = Some(b);
                continue;
            }
        }
        return None;
    }
    match (shape, reference) {
        (Some(s), b) => Some(ClipPath::Shape(s, b)),
        (None, Some(b)) => Some(ClipPath::Box(b)),
        (None, None) => None,
    }
}
