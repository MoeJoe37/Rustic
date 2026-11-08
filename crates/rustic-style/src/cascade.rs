//! The cascade (CSS Cascade Level 5 §6) and inheritance (§7).
//!
//! For each element and longhand, the winning declaration is chosen by, in
//! order: origin and importance; whether it comes from the element's `style`
//! attribute; selector specificity; and order of appearance. CSS-wide
//! keywords are then resolved: `initial`, `inherit`, `unset`, and `revert` /
//! `revert-layer` (which roll back to the previous origin, since cascade
//! layers are not implemented yet). Properties with no declaration inherit or
//! take their initial value.
//!
//! The result is the *specified* value of every supported longhand; inherited
//! values come from the parent's [`ComputedStyle`], and
//! [`ComputedStyle::compute`] turns the result into computed values.

use rustic_css::{
    CssWideKeyword, DeclaredValue, MediaEnvironment, PropertyDeclaration, PropertyId,
    PropertyValue, Specificity,
};

use crate::matching::{MatchCache, matches_complex_cached};
use crate::{ComputedStyle, ElementView, MatchContext, Origin, StyleSheet};

/// The specified value of every supported longhand for one element.
#[derive(Clone, Debug, PartialEq)]
pub struct SpecifiedStyle {
    values: Vec<PropertyValue>,
}

fn index(property: PropertyId) -> usize {
    PropertyId::ALL
        .iter()
        .position(|&p| p == property)
        .expect("every PropertyId is listed in ALL")
}

impl SpecifiedStyle {
    /// Initial values for every longhand (the style of a root's parent).
    pub fn initial() -> Self {
        Self {
            values: PropertyId::ALL.iter().map(|p| p.initial_value()).collect(),
        }
    }

    /// The specified value of `property`.
    pub fn get(&self, property: PropertyId) -> &PropertyValue {
        &self.values[index(property)]
    }

    /// `property: value` lines for every longhand that differs from its
    /// initial value, in [`PropertyId::ALL`] order.
    ///
    /// A value equal to the *computed* initial value (such as the `16px`
    /// font size a root passes down by inheritance) also counts as initial.
    pub fn dump_non_initial(&self) -> String {
        let computed = ComputedStyle::initial();
        let mut out = String::new();
        for (&property, value) in PropertyId::ALL.iter().zip(&self.values) {
            if *value != property.initial_value() && value != computed.get(property) {
                out.push_str(property.name());
                out.push_str(": ");
                out.push_str(&value.to_string());
                out.push('\n');
            }
        }
        out
    }
}

/// Cascade priority: higher sorts first.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Priority {
    /// Origin and importance (§6.2): normal UA < normal user < normal author
    /// < important author < important user < important UA.
    tier: u8,
    /// Declarations from the `style` attribute beat rule declarations.
    inline: bool,
    specificity: Specificity,
    /// Order of appearance across all sheets, then within the block.
    order: (usize, usize, usize),
}

fn tier(origin: Origin, important: bool) -> u8 {
    match (origin, important) {
        (Origin::UserAgent, false) => 0,
        (Origin::User, false) => 1,
        (Origin::Author, false) => 2,
        (Origin::Author, true) => 3,
        (Origin::User, true) => 4,
        (Origin::UserAgent, true) => 5,
    }
}

struct Candidate<'a> {
    priority: Priority,
    origin: Origin,
    declaration: &'a PropertyDeclaration,
}

/// An ordered set of stylesheets from any origins, evaluated against one
/// media environment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cascade {
    sheets: Vec<StyleSheet>,
    environment: MediaEnvironment,
    /// Per sheet, whether each of its media query lists matches.
    media_results: Vec<Vec<bool>>,
}

fn evaluate(sheet: &StyleSheet, environment: &MediaEnvironment) -> Vec<bool> {
    sheet.media.iter().map(|m| m.matches(environment)).collect()
}

impl Cascade {
    /// An empty cascade for the default environment (a 1280×720 screen).
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty cascade for `environment`.
    pub fn with_environment(environment: MediaEnvironment) -> Self {
        Self {
            environment,
            ..Self::default()
        }
    }

    /// Append a stylesheet; later sheets come later in order of appearance.
    pub fn push(&mut self, sheet: StyleSheet) {
        self.media_results.push(evaluate(&sheet, &self.environment));
        self.sheets.push(sheet);
    }

    /// The stylesheets in order.
    pub fn sheets(&self) -> &[StyleSheet] {
        &self.sheets
    }

    /// The media environment rules are evaluated against.
    pub fn environment(&self) -> &MediaEnvironment {
        &self.environment
    }

    /// Switch to `environment` (for example after a viewport resize) and
    /// re-evaluate every media query. Returns whether any media query list
    /// changed its result, which is when styles may need recomputing.
    pub fn set_environment(&mut self, environment: MediaEnvironment) -> bool {
        let results: Vec<Vec<bool>> = self
            .sheets
            .iter()
            .map(|sheet| evaluate(sheet, &environment))
            .collect();
        let changed = results != self.media_results;
        self.environment = environment;
        self.media_results = results;
        changed
    }

    /// Compute `element`'s specified style.
    ///
    /// `inline` holds the element's `style` attribute declarations (author
    /// origin); `parent` is the parent element's computed style, or `None`
    /// for a root, which inherits computed initial values. Inherited values
    /// and `inherit` take the parent's computed value, as CSS requires.
    pub fn specified_style<E: ElementView>(
        &self,
        element: E,
        cx: &MatchContext<E>,
        inline: &[PropertyDeclaration],
        parent: Option<&ComputedStyle>,
    ) -> SpecifiedStyle {
        self.specified_style_cached(element, cx, inline, parent, &MatchCache::default())
    }

    /// [`Self::specified_style`] sharing `cache` across one restyle.
    pub(crate) fn specified_style_cached<E: ElementView>(
        &self,
        element: E,
        cx: &MatchContext<E>,
        inline: &[PropertyDeclaration],
        parent: Option<&ComputedStyle>,
        cache: &MatchCache<E>,
    ) -> SpecifiedStyle {
        let mut candidates: Vec<Candidate<'_>> = Vec::new();
        for (sheet_index, (sheet, media)) in self.sheets.iter().zip(&self.media_results).enumerate()
        {
            for (rule_index, rule) in sheet.rules.iter().enumerate() {
                if !StyleSheet::rule_applies(rule, media) {
                    continue;
                }
                let specificity = rule
                    .selectors
                    .selectors()
                    .iter()
                    .filter(|s| {
                        s.pseudo_element().is_none()
                            && matches_complex_cached(s, element, cx, cache)
                    })
                    .map(|s| s.specificity())
                    .max();
                let Some(specificity) = specificity else {
                    continue;
                };
                for (declaration_index, declaration) in rule.declarations.iter().enumerate() {
                    candidates.push(Candidate {
                        priority: Priority {
                            tier: tier(sheet.origin, declaration.important),
                            inline: false,
                            specificity,
                            order: (sheet_index, rule_index, declaration_index),
                        },
                        origin: sheet.origin,
                        declaration,
                    });
                }
            }
        }
        for (declaration_index, declaration) in inline.iter().enumerate() {
            candidates.push(Candidate {
                priority: Priority {
                    tier: tier(Origin::Author, declaration.important),
                    inline: true,
                    specificity: Specificity::default(),
                    order: (usize::MAX, 0, declaration_index),
                },
                origin: Origin::Author,
                declaration,
            });
        }
        candidates.sort_by_key(|c| std::cmp::Reverse(c.priority));

        let initial;
        let parent = match parent {
            Some(parent) => parent,
            None => {
                initial = ComputedStyle::initial();
                &initial
            }
        };
        let values = PropertyId::ALL
            .iter()
            .map(|&property| {
                let inherit = || parent.get(property).clone();
                let unset = || {
                    if property.inherited() {
                        inherit()
                    } else {
                        property.initial_value()
                    }
                };
                // Origins excluded by `revert`; anything at or above is skipped.
                let mut ceiling: Option<Origin> = None;
                for candidate in candidates
                    .iter()
                    .filter(|c| c.declaration.property == property)
                {
                    if ceiling.is_some_and(|c| candidate.origin >= c) {
                        continue;
                    }
                    match &candidate.declaration.value {
                        DeclaredValue::Value(value) => return value.clone(),
                        DeclaredValue::CssWide(CssWideKeyword::Initial) => {
                            return property.initial_value();
                        }
                        DeclaredValue::CssWide(CssWideKeyword::Inherit) => return inherit(),
                        DeclaredValue::CssWide(CssWideKeyword::Unset) => return unset(),
                        DeclaredValue::CssWide(
                            CssWideKeyword::Revert | CssWideKeyword::RevertLayer,
                        ) => {
                            if candidate.origin == Origin::UserAgent {
                                return unset();
                            }
                            ceiling = Some(candidate.origin);
                        }
                    }
                }
                unset()
            })
            .collect();
        SpecifiedStyle { values }
    }
}
