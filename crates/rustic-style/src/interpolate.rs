//! Interpolation and addition of computed values (CSS Values 4 §3
//! "Combining values", with each property's "Animation type" line, CSS
//! Transforms 2 §§10–11, CSS Color 4 §12; editor's drafts reviewed
//! 2026-11-25).
//!
//! Values are computed values, so lengths are pixels and colors are never
//! `currentcolor`. A pair that cannot be interpolated (different keywords,
//! mismatched lists that have no common form) falls back to discrete
//! animation: the first value before the midpoint, the second from it.
//!
//! Colors interpolate in premultiplied sRGB when both are legacy `rgb()`
//! colors and in premultiplied Oklab otherwise. Transform lists interpolate
//! function by function when their primitives match and through matrix
//! decomposition otherwise; lists that need decomposition and contain
//! percentage translations are discrete (TD-201).

use rustic_css::{
    BackgroundSize, BorderWidth, Calc, Color, CompositeOperation, CornerRadius, Display,
    FilterFunction, FlexBasis, FontSize, FontStretch, FontWeight, Gap, IndividualTransform,
    LabSpace, Length, LengthPercentage, LengthPercentageOrAuto, LineHeight, MaxSize,
    PositionComponent, PositionValue, PropertyId, PropertyValue, Shadow, Size, Spacing,
    TransformFunction, TransformList, VerticalAlign, Visibility, ZIndex,
};

/// How a property animates (each property's "Animation type").
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnimationType {
    /// Cannot be animated or transitioned at all (`animation-*`,
    /// `transition-*`, `direction`, `unicode-bidi`).
    NotAnimatable,
    /// Flips from one value to the other at the midpoint; transitions only
    /// with `transition-behavior: allow-discrete`.
    Discrete,
    /// Interpolates between values of the same type (pairs of different
    /// types still flip discretely).
    ByComputedValue,
}

/// The animation type of `property`.
pub fn animation_type(property: PropertyId) -> AnimationType {
    use PropertyId as P;
    match property {
        P::AnimationTimingFunction
        | P::TransitionTimingFunction
        | P::TransitionProperty
        | P::TransitionDuration
        | P::TransitionDelay
        | P::TransitionBehavior
        | P::AnimationName
        | P::AnimationDuration
        | P::AnimationDelay
        | P::AnimationIterationCount
        | P::AnimationDirection
        | P::AnimationFillMode
        | P::AnimationPlayState
        | P::AnimationComposition
        | P::AnimationTimeline
        | P::Direction
        | P::UnicodeBidi => AnimationType::NotAnimatable,
        P::Color
        | P::BackgroundColor
        | P::BorderTopColor
        | P::BorderRightColor
        | P::BorderBottomColor
        | P::BorderLeftColor
        | P::TextDecorationColor
        | P::OutlineColor
        | P::MarginTop
        | P::MarginRight
        | P::MarginBottom
        | P::MarginLeft
        | P::PaddingTop
        | P::PaddingRight
        | P::PaddingBottom
        | P::PaddingLeft
        | P::BorderTopWidth
        | P::BorderRightWidth
        | P::BorderBottomWidth
        | P::BorderLeftWidth
        | P::OutlineWidth
        | P::OutlineOffset
        | P::Width
        | P::Height
        | P::MinWidth
        | P::MinHeight
        | P::MaxWidth
        | P::MaxHeight
        | P::Top
        | P::Right
        | P::Bottom
        | P::Left
        | P::FontSize
        | P::FontWeight
        | P::FontStretch
        | P::LineHeight
        | P::LetterSpacing
        | P::WordSpacing
        | P::TextIndent
        | P::VerticalAlign
        | P::ZIndex
        | P::Opacity
        | P::Visibility
        | P::BorderTopLeftRadius
        | P::BorderTopRightRadius
        | P::BorderBottomRightRadius
        | P::BorderBottomLeftRadius
        | P::Transform
        | P::Translate
        | P::Rotate
        | P::Scale
        | P::TransformOrigin
        | P::Perspective
        | P::PerspectiveOrigin
        | P::BoxShadow
        | P::Filter
        | P::BackgroundPosition
        | P::BackgroundSize
        | P::FlexGrow
        | P::FlexShrink
        | P::FlexBasis
        | P::Order
        | P::RowGap
        | P::ColumnGap
        | P::BorderSpacing => AnimationType::ByComputedValue,
        _ => AnimationType::Discrete,
    }
}

/// Discrete animation: `from` before the midpoint, `to` from it
/// (CSS Values 4 §3.1), with the special cases of `visibility` and
/// `display` (a `visible` or non-`none` endpoint holds between them).
pub fn discrete(
    property: PropertyId,
    from: &PropertyValue,
    to: &PropertyValue,
    progress: f64,
) -> PropertyValue {
    let pick = |p: f64| if p < 0.5 { from.clone() } else { to.clone() };
    match (property, from, to) {
        (PropertyId::Visibility, PropertyValue::Visibility(a), PropertyValue::Visibility(b))
            if (*a == Visibility::Visible) != (*b == Visibility::Visible) =>
        {
            if progress <= 0.0 {
                from.clone()
            } else if progress >= 1.0 {
                to.clone()
            } else {
                PropertyValue::Visibility(Visibility::Visible)
            }
        }
        (PropertyId::Display, PropertyValue::Display(a), PropertyValue::Display(b))
            if (*a == Display::None) != (*b == Display::None) =>
        {
            // Between the endpoints the value that is not `none` shows.
            let show_to = progress >= 1.0 || (progress > 0.0 && *a == Display::None);
            if show_to { to.clone() } else { from.clone() }
        }
        _ => pick(progress),
    }
}

/// Interpolate two computed values of `property` at `progress` (which may
/// lie outside `0..=1` with overshooting easing). Pairs that cannot be
/// interpolated animate discretely; results are clamped to the property's
/// range.
pub fn interpolate(
    property: PropertyId,
    from: &PropertyValue,
    to: &PropertyValue,
    progress: f64,
) -> PropertyValue {
    if animation_type(property) == AnimationType::ByComputedValue {
        if let Some(value) = try_interpolate(property, from, to, progress) {
            return value;
        }
    }
    discrete(property, from, to, progress)
}

/// Whether two computed values of `property` interpolate smoothly (rather
/// than discretely). Transitions need this unless `allow-discrete`.
pub fn interpolable(property: PropertyId, from: &PropertyValue, to: &PropertyValue) -> bool {
    animation_type(property) == AnimationType::ByComputedValue
        && try_interpolate(property, from, to, 0.5).is_some()
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn lp(a: &LengthPercentage, b: &LengthPercentage, t: f64) -> LengthPercentage {
    if a == b {
        return a.clone();
    }
    Calc::mix(a, 1.0 - t, b, t)
}

/// Clamp a single negative length or percentage at zero (a mixed
/// calculation is clamped by whoever resolves it).
fn nonnegative(value: LengthPercentage) -> LengthPercentage {
    match value {
        LengthPercentage::Length(l) if l.value < 0.0 => LengthPercentage::Length(Length::px(0.0)),
        LengthPercentage::Percentage(p) if p < 0.0 => LengthPercentage::Percentage(0.0),
        other => other,
    }
}

fn px(value: &Length) -> Option<f64> {
    value.to_px()
}

fn round_integer(value: f64) -> i32 {
    // Integers interpolate to the nearest integer, halfway values toward
    // positive infinity.
    let rounded = (value + 0.5).floor();
    rounded.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

/// Colors (CSS Color 4 §12): premultiplied sRGB for two legacy colors,
/// premultiplied Oklab otherwise.
pub fn interpolate_color(a: &Color, b: &Color, t: f64) -> Option<Color> {
    if let (
        Color::Rgb {
            red: r1,
            green: g1,
            blue: b1,
            alpha: a1,
        },
        Color::Rgb {
            red: r2,
            green: g2,
            blue: b2,
            alpha: a2,
        },
    ) = (*a, *b)
    {
        let alpha = lerp(a1, a2, t).clamp(0.0, 1.0);
        let channel = |c1: f64, c2: f64| {
            if alpha == 0.0 {
                0.0
            } else {
                (lerp(c1 * a1, c2 * a2, t) / alpha).clamp(0.0, 255.0)
            }
        };
        return Some(Color::Rgb {
            red: channel(r1, r2),
            green: channel(g1, g2),
            blue: channel(b1, b2),
            alpha,
        });
    }
    let (ca, aa) = a.to_oklab()?;
    let (cb, ab) = b.to_oklab()?;
    let alpha = lerp(aa, ab, t).clamp(0.0, 1.0);
    let channel = |i: usize| {
        if alpha == 0.0 {
            0.0
        } else {
            lerp(ca[i] * aa, cb[i] * ab, t) / alpha
        }
    };
    Some(Color::Lab {
        space: LabSpace::Oklab,
        channels: [
            Some(channel(0).max(0.0)),
            Some(channel(1)),
            Some(channel(2)),
        ],
        alpha: Some(alpha),
    })
}

fn try_interpolate(
    property: PropertyId,
    from: &PropertyValue,
    to: &PropertyValue,
    t: f64,
) -> Option<PropertyValue> {
    use PropertyId as P;
    use PropertyValue as V;
    let negative_ok = matches!(
        property,
        P::MarginTop
            | P::MarginRight
            | P::MarginBottom
            | P::MarginLeft
            | P::Top
            | P::Right
            | P::Bottom
            | P::Left
            | P::TextIndent
            | P::OutlineOffset
            | P::LetterSpacing
            | P::WordSpacing
            | P::VerticalAlign
    );
    let clamp = |value: LengthPercentage| {
        if negative_ok {
            value
        } else {
            nonnegative(value)
        }
    };
    Some(match (from, to) {
        (V::Color(a), V::Color(b)) => V::Color(interpolate_color(a, b, t)?),
        (V::LengthPercentageOrAuto(a), V::LengthPercentageOrAuto(b)) => match (a, b) {
            (
                LengthPercentageOrAuto::LengthPercentage(a),
                LengthPercentageOrAuto::LengthPercentage(b),
            ) => V::LengthPercentageOrAuto(LengthPercentageOrAuto::LengthPercentage(clamp(lp(
                a, b, t,
            )))),
            _ => return None,
        },
        (V::LengthPercentage(a), V::LengthPercentage(b)) => V::LengthPercentage(clamp(lp(a, b, t))),
        (V::Size(Size::LengthPercentage(a)), V::Size(Size::LengthPercentage(b))) => {
            V::Size(Size::LengthPercentage(nonnegative(lp(a, b, t))))
        }
        (V::MaxSize(MaxSize::LengthPercentage(a)), V::MaxSize(MaxSize::LengthPercentage(b))) => {
            V::MaxSize(MaxSize::LengthPercentage(nonnegative(lp(a, b, t))))
        }
        (
            V::FlexBasis(FlexBasis::Size(Size::LengthPercentage(a))),
            V::FlexBasis(FlexBasis::Size(Size::LengthPercentage(b))),
        ) => V::FlexBasis(FlexBasis::Size(Size::LengthPercentage(nonnegative(lp(
            a, b, t,
        ))))),
        (V::Gap(Gap::LengthPercentage(a)), V::Gap(Gap::LengthPercentage(b))) => {
            V::Gap(Gap::LengthPercentage(nonnegative(lp(a, b, t))))
        }
        (V::BorderSpacing(ah, av), V::BorderSpacing(bh, bv)) => {
            V::BorderSpacing(nonnegative(lp(ah, bh, t)), nonnegative(lp(av, bv, t)))
        }
        (V::BorderWidth(BorderWidth::Length(a)), V::BorderWidth(BorderWidth::Length(b))) => {
            V::BorderWidth(BorderWidth::Length(Length::px(
                lerp(px(a)?, px(b)?, t).max(0.0),
            )))
        }
        (V::BorderRadius(a), V::BorderRadius(b)) => V::BorderRadius(CornerRadius {
            horizontal: nonnegative(lp(&a.horizontal, &b.horizontal, t)),
            vertical: nonnegative(lp(&a.vertical, &b.vertical, t)),
        }),
        (
            V::FontSize(FontSize::LengthPercentage(a)),
            V::FontSize(FontSize::LengthPercentage(b)),
        ) => V::FontSize(FontSize::LengthPercentage(nonnegative(lp(a, b, t)))),
        (V::FontWeight(FontWeight::Number(a)), V::FontWeight(FontWeight::Number(b))) => {
            V::FontWeight(FontWeight::Number(lerp(*a, *b, t).clamp(1.0, 1000.0)))
        }
        (
            V::FontStretch(FontStretch::Percentage(a)),
            V::FontStretch(FontStretch::Percentage(b)),
        ) => V::FontStretch(FontStretch::Percentage(lerp(*a, *b, t).max(0.0))),
        (V::LineHeight(a), V::LineHeight(b)) => V::LineHeight(match (a, b) {
            (LineHeight::Number(a), LineHeight::Number(b)) => {
                LineHeight::Number(lerp(*a, *b, t).max(0.0))
            }
            (LineHeight::LengthPercentage(a), LineHeight::LengthPercentage(b)) => {
                LineHeight::LengthPercentage(nonnegative(lp(a, b, t)))
            }
            _ => return None,
        }),
        (V::Spacing(Spacing::LengthPercentage(a)), V::Spacing(Spacing::LengthPercentage(b))) => {
            V::Spacing(Spacing::LengthPercentage(lp(a, b, t)))
        }
        (
            V::VerticalAlign(VerticalAlign::LengthPercentage(a)),
            V::VerticalAlign(VerticalAlign::LengthPercentage(b)),
        ) => V::VerticalAlign(VerticalAlign::LengthPercentage(lp(a, b, t))),
        (V::ZIndex(ZIndex::Integer(a)), V::ZIndex(ZIndex::Integer(b))) => V::ZIndex(
            ZIndex::Integer(round_integer(lerp(f64::from(*a), f64::from(*b), t))),
        ),
        (V::Integer(a), V::Integer(b)) => {
            V::Integer(round_integer(lerp(f64::from(*a), f64::from(*b), t)))
        }
        (V::Number(a), V::Number(b)) => V::Number(lerp(*a, *b, t).max(0.0)),
        (V::Opacity(a), V::Opacity(b)) => V::Opacity(lerp(*a, *b, t).clamp(0.0, 1.0)),
        (V::Visibility(a), V::Visibility(b)) => {
            // Interpolable only between `visible` and something else.
            if (*a == Visibility::Visible) == (*b == Visibility::Visible) {
                return None;
            }
            discrete(property, from, to, t)
        }
        (V::TransformOrigin(pa, za), V::TransformOrigin(pb, zb)) => {
            V::TransformOrigin(position(pa, pb, t), lp(za, zb, t))
        }
        (V::PerspectiveOrigin(a), V::PerspectiveOrigin(b)) => {
            V::PerspectiveOrigin(position(a, b, t))
        }
        (V::Perspective(Some(a)), V::Perspective(Some(b))) => {
            V::Perspective(Some(nonnegative(lp(a, b, t))))
        }
        (V::BackgroundPosition(a), V::BackgroundPosition(b)) => {
            V::BackgroundPosition(repeatable(a, b, |a, b| Some(position(a, b, t)))?)
        }
        (V::BackgroundSize(a), V::BackgroundSize(b)) => {
            V::BackgroundSize(repeatable(a, b, |a, b| background_size(a, b, t))?)
        }
        (V::BoxShadow(a), V::BoxShadow(b)) => V::BoxShadow(shadows(a, b, t)?),
        (V::Filter(a), V::Filter(b)) => V::Filter(filters(a, b, t)?),
        (V::Transform(a), V::Transform(b)) => {
            V::Transform(crate::transforms::interpolate_lists(a, b, t)?)
        }
        (V::IndividualTransform(a), V::IndividualTransform(b)) => {
            V::IndividualTransform(individual(property, a, b, t)?)
        }
        _ => return None,
    })
}

fn position(a: &PositionValue, b: &PositionValue, t: f64) -> PositionValue {
    PositionValue {
        x: PositionComponent::Offset(lp(&a.x.to_offset(), &b.x.to_offset(), t)),
        y: PositionComponent::Offset(lp(&a.y.to_offset(), &b.y.to_offset(), t)),
    }
}

fn background_size(a: &BackgroundSize, b: &BackgroundSize, t: f64) -> Option<BackgroundSize> {
    match (a, b) {
        (BackgroundSize::Explicit(w1, h1), BackgroundSize::Explicit(w2, h2)) => {
            let side = |x: &Option<LengthPercentage>, y: &Option<LengthPercentage>| match (x, y) {
                (Some(x), Some(y)) => Some(Some(nonnegative(lp(x, y, t)))),
                (None, None) => Some(None),
                _ => None,
            };
            Some(BackgroundSize::Explicit(side(w1, w2)?, side(h1, h2)?))
        }
        (a, b) if a == b => Some(a.clone()),
        _ => None,
    }
}

/// Repeatable lists (CSS Values 4 §3.2): both are repeated to their least
/// common multiple length and combined item by item.
fn repeatable<T>(a: &[T], b: &[T], item: impl Fn(&T, &T) -> Option<T>) -> Option<Vec<T>> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let gcd = |mut x: usize, mut y: usize| {
        while y != 0 {
            (x, y) = (y, x % y);
        }
        x
    };
    let len = a.len() / gcd(a.len(), b.len()) * b.len();
    if len > 1024 {
        return None;
    }
    (0..len)
        .map(|i| item(&a[i % a.len()], &b[i % b.len()]))
        .collect()
}

fn transparent_shadow(like: &Shadow) -> Shadow {
    Shadow {
        color: Some(Color::Rgb {
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            alpha: 0.0,
        }),
        x: LengthPercentage::ZERO,
        y: LengthPercentage::ZERO,
        blur: Some(LengthPercentage::ZERO),
        spread: like.spread.as_ref().map(|_| LengthPercentage::ZERO),
        inset: like.inset,
    }
}

fn shadow(a: &Shadow, b: &Shadow, t: f64) -> Option<Shadow> {
    if a.inset != b.inset {
        return None;
    }
    let zero = LengthPercentage::ZERO;
    let opt = |x: &Option<LengthPercentage>, y: &Option<LengthPercentage>| {
        lp(x.as_ref().unwrap_or(&zero), y.as_ref().unwrap_or(&zero), t)
    };
    let black = Color::rgb(0, 0, 0);
    Some(Shadow {
        color: Some(interpolate_color(
            a.color.as_ref().unwrap_or(&black),
            b.color.as_ref().unwrap_or(&black),
            t,
        )?),
        x: lp(&a.x, &b.x, t),
        y: lp(&a.y, &b.y, t),
        blur: Some(nonnegative(opt(&a.blur, &b.blur))),
        spread: (a.spread.is_some() || b.spread.is_some()).then(|| opt(&a.spread, &b.spread)),
        inset: a.inset,
    })
}

/// Shadow lists: the shorter list is padded with transparent shadows.
fn shadows(a: &[Shadow], b: &[Shadow], t: f64) -> Option<Vec<Shadow>> {
    let len = a.len().max(b.len());
    (0..len)
        .map(|i| match (a.get(i), b.get(i)) {
            (Some(x), Some(y)) => shadow(x, y, t),
            (Some(x), None) => shadow(x, &transparent_shadow(x), t),
            (None, Some(y)) => shadow(&transparent_shadow(y), y, t),
            (None, None) => None,
        })
        .collect()
}

/// The identity value of a filter function's kind (Filter Effects 1
/// §12), for padding the shorter list.
fn filter_identity(like: &FilterFunction) -> Option<FilterFunction> {
    use FilterFunction as F;
    Some(match like {
        F::Blur(_) => F::Blur(LengthPercentage::ZERO),
        F::Brightness(_) => F::Brightness(1.0),
        F::Contrast(_) => F::Contrast(1.0),
        F::Grayscale(_) => F::Grayscale(0.0),
        F::HueRotate(_) => F::HueRotate(0.0),
        F::Invert(_) => F::Invert(0.0),
        F::Opacity(_) => F::Opacity(1.0),
        F::Saturate(_) => F::Saturate(1.0),
        F::Sepia(_) => F::Sepia(0.0),
        F::DropShadow(s) => F::DropShadow(transparent_shadow(s)),
        F::Url(_) => return None,
    })
}

fn filter(a: &FilterFunction, b: &FilterFunction, t: f64) -> Option<FilterFunction> {
    use FilterFunction as F;
    let unit = |x: f64, y: f64| lerp(x, y, t).clamp(0.0, 1.0);
    let positive = |x: f64, y: f64| lerp(x, y, t).max(0.0);
    Some(match (a, b) {
        (F::Blur(x), F::Blur(y)) => F::Blur(nonnegative(lp(x, y, t))),
        (F::Brightness(x), F::Brightness(y)) => F::Brightness(positive(*x, *y)),
        (F::Contrast(x), F::Contrast(y)) => F::Contrast(positive(*x, *y)),
        (F::Grayscale(x), F::Grayscale(y)) => F::Grayscale(unit(*x, *y)),
        (F::HueRotate(x), F::HueRotate(y)) => F::HueRotate(lerp(*x, *y, t)),
        (F::Invert(x), F::Invert(y)) => F::Invert(unit(*x, *y)),
        (F::Opacity(x), F::Opacity(y)) => F::Opacity(unit(*x, *y)),
        (F::Saturate(x), F::Saturate(y)) => F::Saturate(positive(*x, *y)),
        (F::Sepia(x), F::Sepia(y)) => F::Sepia(unit(*x, *y)),
        (F::DropShadow(x), F::DropShadow(y)) => F::DropShadow(shadow(x, y, t)?),
        _ => return None,
    })
}

/// Filter lists (Filter Effects 1 §11): matching kinds function by
/// function, the shorter list padded with identity functions.
fn filters(a: &[FilterFunction], b: &[FilterFunction], t: f64) -> Option<Vec<FilterFunction>> {
    let len = a.len().max(b.len());
    (0..len)
        .map(|i| match (a.get(i), b.get(i)) {
            (Some(x), Some(y)) => filter(x, y, t),
            (Some(x), None) => filter(x, &filter_identity(x)?, t),
            (None, Some(y)) => filter(&filter_identity(y)?, y, t),
            (None, None) => None,
        })
        .collect()
}

/// `translate`, `rotate` and `scale` (CSS Transforms 2 §11): `none` acts
/// as the identity of the other side.
fn individual(
    property: PropertyId,
    a: &IndividualTransform,
    b: &IndividualTransform,
    t: f64,
) -> Option<IndividualTransform> {
    let identity = |like: &TransformFunction| match like {
        TransformFunction::Rotate { x, y, z, .. } => TransformFunction::Rotate {
            x: *x,
            y: *y,
            z: *z,
            angle: 0.0,
        },
        TransformFunction::Scale(..) => TransformFunction::Scale(1.0, 1.0, 1.0),
        _ => TransformFunction::Translate(
            LengthPercentage::ZERO,
            LengthPercentage::ZERO,
            LengthPercentage::ZERO,
        ),
    };
    let _ = property;
    match (&a.0, &b.0) {
        (None, None) => Some(IndividualTransform(None)),
        (Some(x), None) => Some(IndividualTransform(Some(
            crate::transforms::interpolate_function(x, &identity(x), t)?,
        ))),
        (None, Some(y)) => Some(IndividualTransform(Some(
            crate::transforms::interpolate_function(&identity(y), y, t)?,
        ))),
        (Some(x), Some(y)) => Some(IndividualTransform(Some(
            crate::transforms::interpolate_function(x, y, t)?,
        ))),
    }
}

/// Combine `value` onto `underlying` for a composite operation (CSS
/// Values 4 §3.3, Web Animations §5.4.4). Values that do not add (or
/// pairs of different types) replace the underlying value.
pub fn composite(
    property: PropertyId,
    underlying: &PropertyValue,
    value: &PropertyValue,
    operation: CompositeOperation,
) -> PropertyValue {
    if operation == CompositeOperation::Replace
        || animation_type(property) != AnimationType::ByComputedValue
    {
        return value.clone();
    }
    use PropertyValue as V;
    // Transform lists and filter lists append under `add`.
    if operation == CompositeOperation::Add {
        match (underlying, value) {
            (V::Transform(a), V::Transform(b)) => {
                let mut list = a.0.clone();
                list.extend(b.0.iter().cloned());
                return V::Transform(TransformList(list));
            }
            (V::Filter(a), V::Filter(b)) => {
                let mut list = a.clone();
                list.extend(b.iter().cloned());
                return V::Filter(list);
            }
            (V::BoxShadow(a), V::BoxShadow(b)) => {
                let mut list = a.clone();
                list.extend(b.iter().cloned());
                return V::BoxShadow(list);
            }
            _ => {}
        }
    }
    // Everything else adds as the sum of the two values: the
    // interpolation `underlying × 1 + value × 1` with extrapolation.
    sum(property, underlying, value).unwrap_or_else(|| value.clone())
}

/// `a + b` for numeric values: interpolation weights 1 and 1, which every
/// linear combination above reduces to, except colors (channel sums,
/// clamped) and transforms (matrix product through accumulation).
fn sum(property: PropertyId, a: &PropertyValue, b: &PropertyValue) -> Option<PropertyValue> {
    use PropertyValue as V;
    match (a, b) {
        (V::Color(x), V::Color(y)) => {
            let ((cx, ax), (cy, ay)) = (x.to_srgb()?, y.to_srgb()?);
            let c = |i: usize| ((cx[i] + cy[i]) * 255.0).clamp(0.0, 255.0);
            Some(V::Color(Color::Rgb {
                red: c(0),
                green: c(1),
                blue: c(2),
                alpha: (ax + ay).clamp(0.0, 1.0),
            }))
        }
        (V::Transform(x), V::Transform(y)) => {
            crate::transforms::accumulate_lists(x, y).map(V::Transform)
        }
        (V::IndividualTransform(x), V::IndividualTransform(y)) => {
            crate::transforms::accumulate_individual(x, y).map(V::IndividualTransform)
        }
        (V::Visibility(_), _) => None,
        _ => {
            // a + b = 2 × interpolate(a, b, 0.5), done by extrapolating:
            // interpolate(a, b, t) = a + (b - a)t, so with a' = a and the
            // "from" weight 1, use mix weights directly where possible.
            let half = try_interpolate(property, a, b, 0.5)?;
            scale(&half, 2.0)
        }
    }
}

/// Multiply a numeric computed value by `factor`.
fn scale(value: &PropertyValue, factor: f64) -> Option<PropertyValue> {
    use PropertyValue as V;
    let zero = LengthPercentage::ZERO;
    let s = |l: &LengthPercentage| Calc::mix(l, factor, &zero, 0.0);
    Some(match value {
        V::LengthPercentageOrAuto(LengthPercentageOrAuto::LengthPercentage(l)) => {
            V::LengthPercentageOrAuto(LengthPercentageOrAuto::LengthPercentage(s(l)))
        }
        V::LengthPercentage(l) => V::LengthPercentage(s(l)),
        V::Size(Size::LengthPercentage(l)) => V::Size(Size::LengthPercentage(s(l))),
        V::MaxSize(MaxSize::LengthPercentage(l)) => V::MaxSize(MaxSize::LengthPercentage(s(l))),
        V::FlexBasis(FlexBasis::Size(Size::LengthPercentage(l))) => {
            V::FlexBasis(FlexBasis::Size(Size::LengthPercentage(s(l))))
        }
        V::Gap(Gap::LengthPercentage(l)) => V::Gap(Gap::LengthPercentage(s(l))),
        V::BorderSpacing(h, v) => V::BorderSpacing(s(h), s(v)),
        V::BorderWidth(BorderWidth::Length(l)) => {
            V::BorderWidth(BorderWidth::Length(Length::px(l.to_px()? * factor)))
        }
        V::BorderRadius(r) => V::BorderRadius(CornerRadius {
            horizontal: s(&r.horizontal),
            vertical: s(&r.vertical),
        }),
        V::FontSize(FontSize::LengthPercentage(l)) => V::FontSize(FontSize::LengthPercentage(s(l))),
        V::FontWeight(FontWeight::Number(n)) => {
            V::FontWeight(FontWeight::Number((n * factor).clamp(1.0, 1000.0)))
        }
        V::FontStretch(FontStretch::Percentage(p)) => {
            V::FontStretch(FontStretch::Percentage(p * factor))
        }
        V::LineHeight(LineHeight::Number(n)) => V::LineHeight(LineHeight::Number(n * factor)),
        V::LineHeight(LineHeight::LengthPercentage(l)) => {
            V::LineHeight(LineHeight::LengthPercentage(s(l)))
        }
        V::Spacing(Spacing::LengthPercentage(l)) => V::Spacing(Spacing::LengthPercentage(s(l))),
        V::VerticalAlign(VerticalAlign::LengthPercentage(l)) => {
            V::VerticalAlign(VerticalAlign::LengthPercentage(s(l)))
        }
        V::ZIndex(ZIndex::Integer(i)) => {
            V::ZIndex(ZIndex::Integer(round_integer(f64::from(*i) * factor)))
        }
        V::Integer(i) => V::Integer(round_integer(f64::from(*i) * factor)),
        V::Number(n) => V::Number(n * factor),
        V::Opacity(n) => V::Opacity((n * factor).clamp(0.0, 1.0)),
        V::TransformOrigin(p, z) => V::TransformOrigin(
            PositionValue {
                x: PositionComponent::Offset(s(&p.x.to_offset())),
                y: PositionComponent::Offset(s(&p.y.to_offset())),
            },
            s(z),
        ),
        V::PerspectiveOrigin(p) => V::PerspectiveOrigin(PositionValue {
            x: PositionComponent::Offset(s(&p.x.to_offset())),
            y: PositionComponent::Offset(s(&p.y.to_offset())),
        }),
        V::Perspective(Some(l)) => V::Perspective(Some(s(l))),
        V::BackgroundPosition(list) => V::BackgroundPosition(
            list.iter()
                .map(|p| PositionValue {
                    x: PositionComponent::Offset(s(&p.x.to_offset())),
                    y: PositionComponent::Offset(s(&p.y.to_offset())),
                })
                .collect(),
        ),
        _ => return None,
    })
}
