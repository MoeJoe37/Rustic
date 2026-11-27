//! Flow-relative (logical) longhands and shorthands (CSS Logical
//! Properties 1) and their mapping to physical longhands (CSS Writing
//! Modes 4 §6).
//!
//! A logical longhand shares the grammar and initial value of its physical
//! counterparts ([`PropertyId::physical_template`]). The cascade resolves
//! each physical longhand together with the logical declarations that map
//! onto it for the element's `writing-mode` and `direction`, so the later
//! of `margin-left` and `margin-inline-start` wins (CSS Logical 1 §4); a
//! logical longhand's computed value is then its physical counterpart's.

use crate::properties::{border_side, border_width, ident, lp_or_auto, nonnegative};
use crate::{BorderStyle, Color, Direction, PropertyId, PropertyValue, Value, WritingMode};

/// A flow-relative side.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Side {
    BlockStart,
    BlockEnd,
    InlineStart,
    InlineEnd,
}

/// What a logical longhand maps onto.
#[derive(Clone, Copy)]
enum Kind {
    /// One of four physical longhands in top, right, bottom, left order.
    Side([PropertyId; 4], Side),
    /// A width (`.0[0]`) or height (`.0[1]`) longhand: the inline size
    /// when `.1`, else the block size.
    Size([PropertyId; 2], bool),
    /// A corner radius between a block side and an inline side.
    Corner(Side, Side),
}

use PropertyId as P;

const MARGIN: [P; 4] = [P::MarginTop, P::MarginRight, P::MarginBottom, P::MarginLeft];
const PADDING: [P; 4] = [
    P::PaddingTop,
    P::PaddingRight,
    P::PaddingBottom,
    P::PaddingLeft,
];
const INSET: [P; 4] = [P::Top, P::Right, P::Bottom, P::Left];
const WIDTH: [P; 4] = [
    P::BorderTopWidth,
    P::BorderRightWidth,
    P::BorderBottomWidth,
    P::BorderLeftWidth,
];
const STYLE: [P; 4] = [
    P::BorderTopStyle,
    P::BorderRightStyle,
    P::BorderBottomStyle,
    P::BorderLeftStyle,
];
const COLOR: [P; 4] = [
    P::BorderTopColor,
    P::BorderRightColor,
    P::BorderBottomColor,
    P::BorderLeftColor,
];

fn kind(property: PropertyId) -> Option<Kind> {
    use Side::{BlockEnd as BE, BlockStart as BS, InlineEnd as IE, InlineStart as IS};
    Some(match property {
        P::BlockSize => Kind::Size([P::Width, P::Height], false),
        P::InlineSize => Kind::Size([P::Width, P::Height], true),
        P::MinBlockSize => Kind::Size([P::MinWidth, P::MinHeight], false),
        P::MinInlineSize => Kind::Size([P::MinWidth, P::MinHeight], true),
        P::MaxBlockSize => Kind::Size([P::MaxWidth, P::MaxHeight], false),
        P::MaxInlineSize => Kind::Size([P::MaxWidth, P::MaxHeight], true),
        P::MarginBlockStart => Kind::Side(MARGIN, BS),
        P::MarginBlockEnd => Kind::Side(MARGIN, BE),
        P::MarginInlineStart => Kind::Side(MARGIN, IS),
        P::MarginInlineEnd => Kind::Side(MARGIN, IE),
        P::PaddingBlockStart => Kind::Side(PADDING, BS),
        P::PaddingBlockEnd => Kind::Side(PADDING, BE),
        P::PaddingInlineStart => Kind::Side(PADDING, IS),
        P::PaddingInlineEnd => Kind::Side(PADDING, IE),
        P::InsetBlockStart => Kind::Side(INSET, BS),
        P::InsetBlockEnd => Kind::Side(INSET, BE),
        P::InsetInlineStart => Kind::Side(INSET, IS),
        P::InsetInlineEnd => Kind::Side(INSET, IE),
        P::BorderBlockStartWidth => Kind::Side(WIDTH, BS),
        P::BorderBlockEndWidth => Kind::Side(WIDTH, BE),
        P::BorderInlineStartWidth => Kind::Side(WIDTH, IS),
        P::BorderInlineEndWidth => Kind::Side(WIDTH, IE),
        P::BorderBlockStartStyle => Kind::Side(STYLE, BS),
        P::BorderBlockEndStyle => Kind::Side(STYLE, BE),
        P::BorderInlineStartStyle => Kind::Side(STYLE, IS),
        P::BorderInlineEndStyle => Kind::Side(STYLE, IE),
        P::BorderBlockStartColor => Kind::Side(COLOR, BS),
        P::BorderBlockEndColor => Kind::Side(COLOR, BE),
        P::BorderInlineStartColor => Kind::Side(COLOR, IS),
        P::BorderInlineEndColor => Kind::Side(COLOR, IE),
        P::BorderStartStartRadius => Kind::Corner(BS, IS),
        P::BorderStartEndRadius => Kind::Corner(BS, IE),
        P::BorderEndStartRadius => Kind::Corner(BE, IS),
        P::BorderEndEndRadius => Kind::Corner(BE, IE),
        _ => return None,
    })
}

/// The physical side (0 top, 1 right, 2 bottom, 3 left) of `side` (CSS
/// Writing Modes 4 §6.4, Table 3).
fn physical_side(side: Side, mode: WritingMode, direction: Direction) -> usize {
    let block_start = match mode {
        WritingMode::HorizontalTb => 0,
        WritingMode::VerticalRl | WritingMode::SidewaysRl => 1,
        WritingMode::VerticalLr | WritingMode::SidewaysLr => 3,
    };
    let inline_start = match (mode, direction) {
        (WritingMode::HorizontalTb, Direction::Ltr) => 3,
        (WritingMode::HorizontalTb, Direction::Rtl) => 1,
        (WritingMode::SidewaysLr, Direction::Ltr) => 2,
        (WritingMode::SidewaysLr, Direction::Rtl) => 0,
        (_, Direction::Ltr) => 0,
        (_, Direction::Rtl) => 2,
    };
    match side {
        Side::BlockStart => block_start,
        Side::BlockEnd => (block_start + 2) % 4,
        Side::InlineStart => inline_start,
        Side::InlineEnd => (inline_start + 2) % 4,
    }
}

impl PropertyId {
    /// Whether this is a flow-relative longhand (`margin-inline-start`,
    /// `block-size`, …).
    pub fn is_logical(self) -> bool {
        kind(self).is_some()
    }

    /// The physical longhand this one sets for an element with `mode` and
    /// `direction`; a physical longhand maps to itself.
    ///
    /// ```
    /// use rustic_css::{Direction, PropertyId, WritingMode};
    /// let start = PropertyId::MarginInlineStart;
    /// assert_eq!(start.to_physical(WritingMode::HorizontalTb, Direction::Rtl), PropertyId::MarginRight);
    /// assert_eq!(start.to_physical(WritingMode::VerticalRl, Direction::Ltr), PropertyId::MarginTop);
    /// assert_eq!(PropertyId::Width.to_physical(WritingMode::VerticalLr, Direction::Ltr), PropertyId::Width);
    /// ```
    pub fn to_physical(self, mode: WritingMode, direction: Direction) -> PropertyId {
        match kind(self) {
            None => self,
            Some(Kind::Side(sides, side)) => sides[physical_side(side, mode, direction)],
            Some(Kind::Size([width, height], inline)) => {
                if inline == mode.is_horizontal() {
                    width
                } else {
                    height
                }
            }
            Some(Kind::Corner(block, inline)) => {
                let sides = [
                    physical_side(block, mode, direction),
                    physical_side(inline, mode, direction),
                ];
                let vertical = sides.iter().find(|s| *s % 2 == 0).copied();
                let horizontal = sides.iter().find(|s| *s % 2 == 1).copied();
                match (vertical, horizontal) {
                    (Some(0), Some(3)) => P::BorderTopLeftRadius,
                    (Some(0), _) => P::BorderTopRightRadius,
                    (_, Some(1)) => P::BorderBottomRightRadius,
                    _ => P::BorderBottomLeftRadius,
                }
            }
        }
    }

    /// The physical longhand whose grammar and initial value a logical
    /// longhand shares (its `horizontal-tb`, `ltr` mapping).
    pub fn physical_template(self) -> PropertyId {
        self.to_physical(WritingMode::HorizontalTb, Direction::Ltr)
    }
}

/// The longhands of a logical shorthand.
pub(crate) fn shorthand_longhands(name: &str) -> Option<&'static [PropertyId]> {
    Some(match name {
        "margin-block" => &[P::MarginBlockStart, P::MarginBlockEnd],
        "margin-inline" => &[P::MarginInlineStart, P::MarginInlineEnd],
        "padding-block" => &[P::PaddingBlockStart, P::PaddingBlockEnd],
        "padding-inline" => &[P::PaddingInlineStart, P::PaddingInlineEnd],
        "inset-block" => &[P::InsetBlockStart, P::InsetBlockEnd],
        "inset-inline" => &[P::InsetInlineStart, P::InsetInlineEnd],
        "border-block-width" => &[P::BorderBlockStartWidth, P::BorderBlockEndWidth],
        "border-inline-width" => &[P::BorderInlineStartWidth, P::BorderInlineEndWidth],
        "border-block-style" => &[P::BorderBlockStartStyle, P::BorderBlockEndStyle],
        "border-inline-style" => &[P::BorderInlineStartStyle, P::BorderInlineEndStyle],
        "border-block-color" => &[P::BorderBlockStartColor, P::BorderBlockEndColor],
        "border-inline-color" => &[P::BorderInlineStartColor, P::BorderInlineEndColor],
        "border-block-start" => &[
            P::BorderBlockStartWidth,
            P::BorderBlockStartStyle,
            P::BorderBlockStartColor,
        ],
        "border-block-end" => &[
            P::BorderBlockEndWidth,
            P::BorderBlockEndStyle,
            P::BorderBlockEndColor,
        ],
        "border-inline-start" => &[
            P::BorderInlineStartWidth,
            P::BorderInlineStartStyle,
            P::BorderInlineStartColor,
        ],
        "border-inline-end" => &[
            P::BorderInlineEndWidth,
            P::BorderInlineEndStyle,
            P::BorderInlineEndColor,
        ],
        "border-block" => &[
            P::BorderBlockStartWidth,
            P::BorderBlockStartStyle,
            P::BorderBlockStartColor,
            P::BorderBlockEndWidth,
            P::BorderBlockEndStyle,
            P::BorderBlockEndColor,
        ],
        "border-inline" => &[
            P::BorderInlineStartWidth,
            P::BorderInlineStartStyle,
            P::BorderInlineStartColor,
            P::BorderInlineEndWidth,
            P::BorderInlineEndStyle,
            P::BorderInlineEndColor,
        ],
        _ => return None,
    })
}

/// One or two values for a start and an end longhand.
fn pair(
    parts: &[&Value],
    one: impl Fn(&Value) -> Option<PropertyValue>,
) -> Option<Vec<PropertyValue>> {
    match parts {
        [both] => {
            let v = one(both)?;
            Some(vec![v.clone(), v])
        }
        [start, end] => Some(vec![one(start)?, one(end)?]),
        _ => None,
    }
}

/// Parse a logical shorthand: `None` for any other name, `Some(None)` for
/// an invalid value.
pub(crate) fn shorthand(name: &str, parts: &[&Value]) -> Option<Option<Vec<PropertyValue>>> {
    let margin = |v: &Value| lp_or_auto(v, true).map(PropertyValue::LengthPercentageOrAuto);
    Some(match name {
        "margin-block" | "margin-inline" | "inset-block" | "inset-inline" => pair(parts, margin),
        "padding-block" | "padding-inline" => pair(parts, |v| {
            nonnegative(v).map(PropertyValue::LengthPercentage)
        }),
        "border-block-width" | "border-inline-width" => {
            pair(parts, |v| border_width(v).map(PropertyValue::BorderWidth))
        }
        "border-block-style" | "border-inline-style" => pair(parts, |v| {
            BorderStyle::from_name(ident(v)?).map(PropertyValue::BorderStyle)
        }),
        "border-block-color" | "border-inline-color" => {
            pair(parts, |v| Color::parse(v).map(PropertyValue::Color))
        }
        "border-block-start" | "border-block-end" | "border-inline-start" | "border-inline-end" => {
            border_side(parts).map(Vec::from)
        }
        "border-block" | "border-inline" => border_side(parts).map(|side| {
            let mut both = Vec::from(side.clone());
            both.extend(side);
            both
        }),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_logical_side_maps_per_writing_mode() {
        use Direction::{Ltr, Rtl};
        use WritingMode::*;
        let s = P::PaddingInlineStart;
        let b = P::PaddingBlockStart;
        let cases = [
            (HorizontalTb, Ltr, P::PaddingLeft, P::PaddingTop),
            (HorizontalTb, Rtl, P::PaddingRight, P::PaddingTop),
            (VerticalRl, Ltr, P::PaddingTop, P::PaddingRight),
            (VerticalRl, Rtl, P::PaddingBottom, P::PaddingRight),
            (VerticalLr, Ltr, P::PaddingTop, P::PaddingLeft),
            (SidewaysRl, Rtl, P::PaddingBottom, P::PaddingRight),
            (SidewaysLr, Ltr, P::PaddingBottom, P::PaddingLeft),
            (SidewaysLr, Rtl, P::PaddingTop, P::PaddingLeft),
        ];
        for (mode, direction, inline, block) in cases {
            assert_eq!(
                s.to_physical(mode, direction),
                inline,
                "{mode} {direction:?}"
            );
            assert_eq!(
                b.to_physical(mode, direction),
                block,
                "{mode} {direction:?}"
            );
        }
        assert_eq!(P::InlineSize.to_physical(VerticalRl, Ltr), P::Height);
        assert_eq!(P::MaxBlockSize.to_physical(VerticalLr, Ltr), P::MaxWidth);
        let corner = P::BorderStartEndRadius;
        assert_eq!(
            corner.to_physical(HorizontalTb, Ltr),
            P::BorderTopRightRadius
        );
        assert_eq!(
            corner.to_physical(VerticalRl, Ltr),
            P::BorderBottomRightRadius
        );
        assert_eq!(corner.to_physical(VerticalLr, Rtl), P::BorderTopLeftRadius);
        for &p in PropertyId::ALL {
            assert!(!p.physical_template().is_logical());
        }
    }
}
