//! A set of loaded faces and CSS font matching (CSS Fonts 4 §5.2).

use crate::error::FontError;
use crate::face::{Face, FaceStyle, FontLimits};

/// One entry of a CSS `font-family` list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FamilyRef<'a> {
    /// A named family, matched ASCII case-insensitively.
    Named(&'a str),
    /// A generic family keyword such as `serif` or `monospace`.
    Generic(&'a str),
}

/// The style descriptors a face is matched against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceQuery {
    /// Desired weight (`font-weight`, 1–1000).
    pub weight: f64,
    /// Desired slope.
    pub style: FaceStyle,
    /// Desired width (`font-stretch` percentage).
    pub stretch: f64,
}

impl Default for FaceQuery {
    fn default() -> Self {
        Self {
            weight: 400.0,
            style: FaceStyle::Normal,
            stretch: 100.0,
        }
    }
}

/// Loaded faces plus generic-family and last-resort mappings.
///
/// There is no system font discovery here (that is `rustic-platform`
/// work); the embedder or test adds faces explicitly.
#[derive(Debug, Default)]
pub struct FontDatabase {
    faces: Vec<Face>,
    generics: Vec<(String, String)>,
    fallback: Option<String>,
}

impl FontDatabase {
    /// An empty database.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse and add a font, returning its index.
    pub fn load(&mut self, data: Vec<u8>, limits: &FontLimits) -> Result<usize, FontError> {
        let face = Face::parse(data, limits)?;
        self.faces.push(face);
        Ok(self.faces.len() - 1)
    }

    /// Every face, in load order.
    pub fn faces(&self) -> &[Face] {
        &self.faces
    }

    /// Map a generic family keyword (for example `sans-serif`) to a family.
    pub fn set_generic_family(&mut self, generic: &str, family: &str) {
        self.generics
            .retain(|(g, _)| !g.eq_ignore_ascii_case(generic));
        self.generics.push((generic.to_owned(), family.to_owned()));
    }

    /// The family used when no `font-family` entry matches any face.
    pub fn set_fallback_family(&mut self, family: &str) {
        self.fallback = Some(family.to_owned());
    }

    fn family_faces(&self, family: &str) -> Vec<&Face> {
        self.faces
            .iter()
            .filter(|face| {
                face.families()
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(family))
            })
            .collect()
    }

    /// The face CSS would use as the *first available font* for this family
    /// list and style: the first family with any face, then the best face of
    /// it by stretch, style and weight. Falls back to the fallback family.
    pub fn select<'a>(
        &self,
        families: impl IntoIterator<Item = FamilyRef<'a>>,
        query: &FaceQuery,
    ) -> Option<&Face> {
        for family in families {
            let name = match family {
                FamilyRef::Named(name) => name,
                FamilyRef::Generic(generic) => match self
                    .generics
                    .iter()
                    .find(|(g, _)| g.eq_ignore_ascii_case(generic))
                {
                    Some((_, family)) => family.as_str(),
                    None => continue,
                },
            };
            if let Some(face) = best_face(self.family_faces(name), query) {
                return Some(face);
            }
        }
        let fallback = self.fallback.as_deref()?;
        best_face(self.family_faces(fallback), query)
    }
}

/// CSS Fonts 4 §5.2 step 4: narrow by `font-stretch`, then `font-style`,
/// then `font-weight`. Equal candidates keep load order.
pub fn best_face<'f>(
    faces: impl IntoIterator<Item = &'f Face>,
    query: &FaceQuery,
) -> Option<&'f Face> {
    let candidates: Vec<&Face> = faces.into_iter().collect();
    let stretches: Vec<f64> = candidates.iter().map(|f| f.stretch()).collect();
    let stretch = nearest(&stretches, query.stretch, 100.0, 100.0)?;
    let candidates: Vec<&Face> = candidates
        .into_iter()
        .filter(|f| f.stretch() == stretch)
        .collect();
    let order: [FaceStyle; 3] = match query.style {
        FaceStyle::Italic => [FaceStyle::Italic, FaceStyle::Oblique, FaceStyle::Normal],
        FaceStyle::Oblique => [FaceStyle::Oblique, FaceStyle::Italic, FaceStyle::Normal],
        FaceStyle::Normal => [FaceStyle::Normal, FaceStyle::Oblique, FaceStyle::Italic],
    };
    let style = order
        .into_iter()
        .find(|s| candidates.iter().any(|f| f.style() == *s))?;
    let candidates: Vec<&Face> = candidates
        .into_iter()
        .filter(|f| f.style() == style)
        .collect();
    let weights: Vec<f64> = candidates.iter().map(|f| f.weight()).collect();
    let weight = nearest(&weights, query.weight, 400.0, 500.0)?;
    candidates.into_iter().find(|f| f.weight() == weight)
}

/// The shared shape of the stretch and weight rules. With `desired` inside
/// `[low, high]`, values from `desired` up to `high` are tried ascending,
/// then values below `desired` descending, then values above `high`
/// ascending. Below `low`, lower values (descending) come first, then
/// higher (ascending); above `high`, higher first, then lower. For stretch
/// `low == high == 100`; for weight `400..=500`.
fn nearest(values: &[f64], desired: f64, low: f64, high: f64) -> Option<f64> {
    if values.contains(&desired) {
        return Some(desired);
    }
    let max_below = |limit: f64| {
        values
            .iter()
            .copied()
            .filter(|&v| v < limit)
            .reduce(f64::max)
    };
    let min_above = |limit: f64| {
        values
            .iter()
            .copied()
            .filter(|&v| v > limit)
            .reduce(f64::min)
    };
    if desired >= low && desired <= high {
        values
            .iter()
            .copied()
            .filter(|&v| v > desired && v <= high)
            .reduce(f64::min)
            .or_else(|| max_below(desired))
            .or_else(|| min_above(high))
    } else if desired < low {
        max_below(desired).or_else(|| min_above(desired))
    } else {
        min_above(desired).or_else(|| max_below(desired))
    }
}

#[cfg(test)]
mod tests {
    use super::nearest;

    #[test]
    fn weight_rules_follow_css_fonts_4() {
        let w = [100.0, 300.0, 600.0, 900.0];
        assert_eq!(nearest(&w, 400.0, 400.0, 500.0), Some(300.0));
        assert_eq!(
            nearest(&[300.0, 450.0, 600.0], 400.0, 400.0, 500.0),
            Some(450.0)
        );
        assert_eq!(nearest(&[300.0, 600.0], 500.0, 400.0, 500.0), Some(300.0));
        assert_eq!(nearest(&[600.0, 800.0], 450.0, 400.0, 500.0), Some(600.0));
        assert_eq!(nearest(&w, 200.0, 400.0, 500.0), Some(100.0));
        assert_eq!(nearest(&[300.0, 600.0], 200.0, 400.0, 500.0), Some(300.0));
        assert_eq!(nearest(&w, 700.0, 400.0, 500.0), Some(900.0));
        assert_eq!(nearest(&[300.0, 600.0], 700.0, 400.0, 500.0), Some(600.0));
        assert_eq!(nearest(&[], 400.0, 400.0, 500.0), None);
    }

    #[test]
    fn stretch_rules_follow_css_fonts_4() {
        let s = [62.5, 75.0, 125.0];
        // Condensed requests prefer narrower, then wider.
        assert_eq!(nearest(&s, 87.5, 100.0, 100.0), Some(75.0));
        assert_eq!(nearest(&[125.0, 150.0], 87.5, 100.0, 100.0), Some(125.0));
        // 100% counts as "at or below normal": narrower first.
        assert_eq!(nearest(&s, 100.0, 100.0, 100.0), Some(75.0));
        // Expanded requests prefer wider, then narrower.
        assert_eq!(nearest(&s, 112.5, 100.0, 100.0), Some(125.0));
        assert_eq!(nearest(&[62.5, 75.0], 150.0, 100.0, 100.0), Some(75.0));
    }
}
