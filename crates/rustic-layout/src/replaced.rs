//! Replaced element sizing (CSS 2.2 §10.3.2, §10.4, §10.6.2; CSS Images 3
//! §5.2 default sizing with the 300×150 fallback object size).
//!
//! Natural dimensions come from the embedder: decoded image sizes are passed
//! in through [`crate::BoxTree::set_natural_sizes`]; `canvas` uses its
//! `width`/`height` attributes. Nothing is decoded or fetched here.

use rustic_css::{BoxSizing, PropertyId as P, PropertyValue as V};
use rustic_style::ComputedStyle;

use crate::intrinsic::{Sizing, has_percentage};
use crate::sizing::{box_edges, lp};
use crate::{CssPx, MeasureError};

/// Natural (intrinsic) dimensions and aspect ratio of replaced content.
///
/// Any part can be missing, as for an SVG with only a `viewBox`, a video
/// before its metadata loads, or an `iframe` (no natural size at all).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NaturalSize {
    width: Option<CssPx>,
    height: Option<CssPx>,
    ratio: Option<f64>,
}

impl NaturalSize {
    /// No natural width, height or ratio (sized 300×150 by default).
    pub const NONE: Self = Self {
        width: None,
        height: None,
        ratio: None,
    };

    /// Natural width and height; a ratio is derived when both are positive.
    pub fn from_dimensions(width: CssPx, height: CssPx) -> Self {
        let ratio = (width.get() > 0.0 && height.get() > 0.0).then(|| width.get() / height.get());
        Self {
            width: Some(width),
            height: Some(height),
            ratio,
        }
    }

    /// Any combination of parts. A ratio must be finite and positive, and
    /// widths/heights are checked CSS pixels; `None` if the ratio is invalid.
    pub fn new(width: Option<CssPx>, height: Option<CssPx>, ratio: Option<f64>) -> Option<Self> {
        if ratio.is_some_and(|r| !(r.is_finite() && r > 0.0)) {
            return None;
        }
        Some(Self {
            width,
            height,
            ratio,
        })
    }

    /// Natural width, if any.
    pub const fn width(self) -> Option<CssPx> {
        self.width
    }

    /// Natural height, if any.
    pub const fn height(self) -> Option<CssPx> {
        self.height
    }

    /// Natural aspect ratio (width / height), if any.
    pub const fn ratio(self) -> Option<f64> {
        self.ratio
    }
}

/// The default object size when no natural size is available.
const DEFAULT: (f64, f64) = (300.0, 150.0);

struct Bounds {
    min: f64,
    max: f64,
}

impl Bounds {
    fn clamp(&self, n: f64) -> f64 {
        n.min(self.max).max(self.min)
    }
}

/// Used content-box width and height of a replaced box.
///
/// `containing_width` is the percentage basis for widths; `containing_height`
/// is the basis for heights, or `None` when it is indefinite (percentage
/// heights then behave as `auto`). Intrinsic sizing keywords on a replaced
/// box size it like `auto`, since its min- and max-content sizes are equal.
pub(crate) fn used_size(
    style: &ComputedStyle,
    natural: NaturalSize,
    containing_width: Option<f64>,
    containing_height: Option<f64>,
) -> Result<(f64, f64), MeasureError> {
    let (padding, border) = box_edges(style, containing_width.unwrap_or(0.0))?;
    let border_box = matches!(style.get(P::BoxSizing), V::BoxSizing(BoxSizing::BorderBox));
    let extras_w =
        padding.left.get() + padding.right.get() + border.left.get() + border.right.get();
    let extras_h =
        padding.top.get() + padding.bottom.get() + border.top.get() + border.bottom.get();
    let read = |p: P, basis: Option<f64>, extras: f64| -> Result<Option<f64>, MeasureError> {
        let Some(value) = Sizing::read(style, p)?.length() else {
            return Ok(None);
        };
        let n = match basis {
            Some(b) => lp(value, b)?,
            None if has_percentage(value) => return Ok(None),
            None => lp(value, 0.0)?,
        };
        Ok(Some(if border_box {
            (n - extras).max(0.0)
        } else {
            n.max(0.0)
        }))
    };
    let width = read(P::Width, containing_width, extras_w)?;
    let height = read(P::Height, containing_height, extras_h)?;
    let w_bounds = {
        let min = read(P::MinWidth, containing_width, extras_w)?.unwrap_or(0.0);
        let max = read(P::MaxWidth, containing_width, extras_w)?.unwrap_or(f64::INFINITY);
        Bounds {
            min,
            max: max.max(min),
        }
    };
    let h_bounds = {
        let min = read(P::MinHeight, containing_height, extras_h)?.unwrap_or(0.0);
        let max = read(P::MaxHeight, containing_height, extras_h)?.unwrap_or(f64::INFINITY);
        Bounds {
            min,
            max: max.max(min),
        }
    };
    let ratio = natural.ratio;
    let nw = natural.width.map(CssPx::get);
    let nh = natural.height.map(CssPx::get);
    Ok(match (width, height) {
        (Some(w), Some(h)) => (w_bounds.clamp(w), h_bounds.clamp(h)),
        (Some(w), None) => {
            let w = w_bounds.clamp(w);
            let h = ratio.map(|r| w / r).or(nh).unwrap_or(DEFAULT.1);
            (w, h_bounds.clamp(h))
        }
        (None, Some(h)) => {
            let w = ratio.map(|r| h * r).or(nw).unwrap_or(DEFAULT.0);
            (w_bounds.clamp(w), h_bounds.clamp(h))
        }
        (None, None) => {
            let w = nw
                .or_else(|| Some(nh? * ratio?))
                .or_else(|| {
                    // Ratio only: fill the containing block (CSS 2.2 leaves
                    // this undefined; this is CSS Sizing's stretch-fit).
                    ratio.and(containing_width.map(|cw| (cw - extras_w).max(0.0)))
                })
                .unwrap_or(DEFAULT.0);
            let h = nh.or_else(|| Some(w / ratio?)).unwrap_or(DEFAULT.1);
            match ratio {
                Some(_) => constrain_ratio(w, h, &w_bounds, &h_bounds),
                None => (w_bounds.clamp(w), h_bounds.clamp(h)),
            }
        }
    })
}

/// The CSS 2.2 §10.4 table for `width:auto; height:auto` with a ratio.
fn constrain_ratio(w: f64, h: f64, wb: &Bounds, hb: &Bounds) -> (f64, f64) {
    if w <= 0.0 || h <= 0.0 {
        return (wb.clamp(w), hb.clamp(h));
    }
    let (over_w, under_w) = (w > wb.max, w < wb.min);
    let (over_h, under_h) = (h > hb.max, h < hb.min);
    match (over_w, under_w, over_h, under_h) {
        (true, _, true, _) if wb.max / w <= hb.max / h => (wb.max, hb.min.max(wb.max * h / w)),
        (true, _, true, _) => (wb.min.max(hb.max * w / h), hb.max),
        (_, true, _, true) if wb.min / w <= hb.min / h => (wb.max.min(hb.min * w / h), hb.min),
        (_, true, _, true) => (wb.min, hb.max.min(wb.min * h / w)),
        (_, true, true, _) => (wb.min, hb.max),
        (true, _, _, true) => (wb.max, hb.min),
        (true, _, _, _) => (wb.max, hb.min.max(wb.max * h / w)),
        (_, true, _, _) => (wb.min, hb.max.min(wb.min * h / w)),
        (_, _, true, _) => (wb.min.max(hb.max * w / h), hb.max),
        (_, _, _, true) => (wb.max.min(hb.min * w / h), hb.min),
        _ => (w, h),
    }
}

/// `canvas` natural size from its `width`/`height` attributes (HTML §4.12.5:
/// valid non-negative integers, defaulting to 300 and 150).
pub(crate) fn canvas_size(width: Option<&[u16]>, height: Option<&[u16]>) -> NaturalSize {
    let parse = |value: Option<&[u16]>, default: f64| -> f64 {
        let Some(units) = value else {
            return default;
        };
        // HTML's rules for parsing non-negative integers: skip leading ASCII
        // whitespace, optional '+', then digits.
        let mut rest = units
            .iter()
            .skip_while(|&&u| matches!(u, 9 | 10 | 12 | 13 | 32))
            .peekable();
        if rest.peek() == Some(&&u16::from(b'+')) {
            rest.next();
        }
        let mut n: f64 = 0.0;
        let mut digits = 0usize;
        for &u in rest {
            if !(u16::from(b'0')..=u16::from(b'9')).contains(&u) {
                break;
            }
            n = (n * 10.0 + f64::from(u - u16::from(b'0'))).min(1.0e9);
            digits += 1;
        }
        if digits == 0 { default } else { n }
    };
    let px = |n: f64| CssPx::new(n).unwrap_or(CssPx::ZERO);
    NaturalSize::from_dimensions(px(parse(width, 300.0)), px(parse(height, 150.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(min: f64, max: f64) -> Bounds {
        Bounds { min, max }
    }

    #[test]
    fn ratio_table_keeps_the_ratio_unless_both_bounds_conflict() {
        let inf = f64::INFINITY;
        // 200×100 natural image.
        assert_eq!(
            constrain_ratio(200.0, 100.0, &b(0.0, inf), &b(0.0, inf)),
            (200.0, 100.0)
        );
        assert_eq!(
            constrain_ratio(200.0, 100.0, &b(0.0, 100.0), &b(0.0, inf)),
            (100.0, 50.0)
        );
        assert_eq!(
            constrain_ratio(200.0, 100.0, &b(400.0, inf), &b(0.0, inf)),
            (400.0, 200.0)
        );
        assert_eq!(
            constrain_ratio(200.0, 100.0, &b(0.0, inf), &b(0.0, 50.0)),
            (100.0, 50.0)
        );
        assert_eq!(
            constrain_ratio(200.0, 100.0, &b(0.0, inf), &b(150.0, inf)),
            (300.0, 150.0)
        );
        // Both too large: the tighter bound wins.
        assert_eq!(
            constrain_ratio(200.0, 100.0, &b(0.0, 100.0), &b(0.0, 80.0)),
            (100.0, 50.0)
        );
        assert_eq!(
            constrain_ratio(200.0, 100.0, &b(0.0, 180.0), &b(0.0, 40.0)),
            (80.0, 40.0)
        );
        // Conflicting directions take both bounds and break the ratio.
        assert_eq!(
            constrain_ratio(200.0, 100.0, &b(300.0, inf), &b(0.0, 50.0)),
            (300.0, 50.0)
        );
        assert_eq!(
            constrain_ratio(200.0, 100.0, &b(0.0, 100.0), &b(120.0, inf)),
            (100.0, 120.0)
        );
    }

    #[test]
    fn canvas_attributes_follow_the_non_negative_integer_rules() {
        let u = |s: &str| s.encode_utf16().collect::<Vec<_>>();
        let size = |w: &str, h: &str| {
            let (w, h) = (u(w), u(h));
            let n = canvas_size(Some(&w), Some(&h));
            (n.width().unwrap().get(), n.height().unwrap().get())
        };
        assert_eq!(size(" +40px", "7.9"), (40.0, 7.0));
        assert_eq!(size("-5", "x"), (300.0, 150.0));
        let n = canvas_size(None, None);
        assert_eq!(n.ratio(), Some(2.0));
    }
}
