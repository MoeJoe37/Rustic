//! The cascade (CSS Cascade Level 5 §6) and inheritance (§7).
//!
//! For each element and longhand, the winning declaration is chosen by, in
//! order: origin and importance (with the animation and transition origins);
//! encapsulation context (shadow trees); whether it comes from the element's
//! `style` attribute; cascade layer; selector specificity; scope proximity; and order of
//! appearance. CSS-wide keywords are then resolved: `initial`, `inherit`,
//! `unset`, `revert` (roll back to the previous origin) and `revert-layer`
//! (roll back to the previous layer). Properties with no declaration
//! inherit or take their initial value.
//!
//! The result is the *specified* value of every supported longhand; inherited
//! values come from the parent's [`ComputedStyle`], and
//! [`ComputedStyle::compute`] turns the result into computed values.

use std::cmp::Reverse;
use std::collections::HashMap;

use rustic_css::{
    CssWideKeyword, DeclaredValue, MediaEnvironment, PropertyDeclaration, PropertyId,
    PropertyValue, Specificity,
};

use rustic_url::Url;

use crate::containers::{ContainerInfo, container_list_matches};
use crate::layer::{LayerOrder, UNLAYERED};
use crate::matching::{MatchCache, matches_in_tree};
use crate::variables::{
    CustomOutcome, CustomProperties, CustomResult, FontDependencies, PropertyRegistration,
    Registry, RegistryRef,
};
use crate::{ComputedStyle, ElementView, MatchContext, Origin, StyleSheet};

/// The specified value of every supported longhand for one element.
#[derive(Clone, Debug, PartialEq)]
pub struct SpecifiedStyle {
    values: Vec<PropertyValue>,
    custom: CustomProperties,
}

fn index(property: PropertyId) -> usize {
    PropertyId::ALL
        .iter()
        .position(|&p| p == property)
        .expect("every PropertyId is listed in ALL")
}

impl SpecifiedStyle {
    pub(crate) fn anonymous(parent: &ComputedStyle, display: rustic_css::Display) -> Self {
        let mut values: Vec<_> = PropertyId::ALL
            .iter()
            .map(|&p| {
                if p.inherited() {
                    parent.get(p).clone()
                } else {
                    p.initial_value()
                }
            })
            .collect();
        values[index(PropertyId::Display)] = PropertyValue::Display(display);
        Self {
            values,
            custom: parent.custom_properties().inherit_without_declarations(),
        }
    }

    /// Initial values for every longhand (the style of a root's parent).
    pub fn initial() -> Self {
        Self {
            values: PropertyId::ALL.iter().map(|p| p.initial_value()).collect(),
            custom: CustomProperties::default(),
        }
    }

    /// The specified value of `property`.
    pub fn get(&self, property: PropertyId) -> &PropertyValue {
        &self.values[index(property)]
    }

    /// The element's computed custom properties.
    pub fn custom_properties(&self) -> &CustomProperties {
        &self.custom
    }

    /// `property: value` lines for every longhand that differs from its
    /// initial value, in [`PropertyId::ALL`] order, then every custom
    /// property in name order.
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
        out.push_str(
            &self
                .custom
                .dump_differences(&CustomProperties::default(), ""),
        );
        out
    }
}

/// Identifies one node tree for the cascade: the document tree or one
/// shadow tree. The caller assigns the numbers; the document tree is
/// [`TreeContext::DOCUMENT`].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TreeContext(pub u64);

impl TreeContext {
    /// The document tree.
    pub const DOCUMENT: Self = Self(0);
}

/// The tree an element is in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TreeScope<E> {
    /// The tree's context.
    pub context: TreeContext,
    /// The shadow host whose shadow tree this is; `None` for the document.
    pub host: Option<E>,
    /// Nesting depth in shadow-including tree order: 0 for the document
    /// tree, one more than the host's tree for a shadow tree.
    pub depth: u32,
}

impl<E> TreeScope<E> {
    /// The document tree.
    pub fn document() -> Self {
        Self {
            context: TreeContext::DOCUMENT,
            host: None,
            depth: 0,
        }
    }
}

/// Everything besides stylesheets that the cascade needs for one element.
#[derive(Clone, Copy, Debug)]
pub struct StyleInputs<'a, E> {
    /// The element's `style` attribute declarations (author origin).
    pub style_attribute: &'a [PropertyDeclaration],
    /// Values from running CSS animations (the animation origin), later
    /// ones winning. Rustic has no animation engine yet, so the engine
    /// passes none; the cascade already places them correctly.
    pub animations: &'a [PropertyDeclaration],
    /// Values from running CSS transitions (the transition origin).
    pub transitions: &'a [PropertyDeclaration],
    /// The tree the element is in.
    pub scope: TreeScope<E>,
    /// For a shadow host, the context of its shadow tree, whose author
    /// sheets style the host through `:host` selectors.
    pub shadow_root: Option<TreeContext>,
    /// The element's ancestors with their computed styles, root first, for
    /// `@container` rules. Without them no container condition holds.
    pub containers: &'a [ContainerInfo<'a, E>],
}

impl<E> StyleInputs<'_, E> {
    /// An element of the document tree with no attached declarations.
    pub fn document() -> Self {
        Self {
            style_attribute: &[],
            animations: &[],
            transitions: &[],
            scope: TreeScope::document(),
            shadow_root: None,
            containers: &[],
        }
    }
}

/// Cascade priority: higher sorts first.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Priority {
    /// Origin and importance (§6.1): normal UA < normal user < normal author
    /// < animations < important author < important user < important UA <
    /// transitions.
    tier: u8,
    /// Encapsulation context: outer wins for normal declarations, inner for
    /// important ones.
    context: u32,
    /// Declarations from the `style` attribute beat rule declarations.
    inline: bool,
    /// Cascade layer: later wins for normal declarations, earlier for
    /// important ones; unlayered declarations are the final layer.
    layer: u32,
    specificity: Specificity,
    /// Closest scope wins among declarations with the same specificity.
    proximity: Reverse<usize>,
    /// Order of appearance across all sheets, then within the block.
    order: (usize, usize, usize),
}

const ANIMATION_TIER: u8 = 3;
const TRANSITION_TIER: u8 = 7;
const UNSCOPED: Reverse<usize> = Reverse(usize::MAX);

fn tier(origin: Origin, important: bool) -> u8 {
    match (origin, important) {
        (Origin::UserAgent, false) => 0,
        (Origin::User, false) => 1,
        (Origin::Author, false) => 2,
        (Origin::Author, true) => 4,
        (Origin::User, true) => 5,
        (Origin::UserAgent, true) => 6,
    }
}

fn context_key(depth: u32, important: bool) -> u32 {
    if important { depth } else { u32::MAX - depth }
}

fn layer_key(rank: u32, important: bool) -> u32 {
    if important { UNLAYERED - rank } else { rank }
}

/// Origins as `revert` sees them: the animation and transition origins
/// count as the author origin.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum RevertLevel {
    UserAgent,
    User,
    Author,
}

struct Candidate<'a> {
    priority: Priority,
    level: RevertLevel,
    /// Context depth and layer rank, to find the normal level of a layer.
    depth: u32,
    rank: u32,
    declaration: &'a PropertyDeclaration,
}

impl Candidate<'_> {
    /// Where `revert-layer` here rolls back to: every candidate at or above
    /// the returned priority is skipped, except for important style
    /// attribute declarations, which skip only style attribute and
    /// animation declarations (`None`).
    fn revert_layer_floor(&self) -> Option<Priority> {
        let p = self.priority;
        let min = |tier| Priority {
            tier,
            context: 0,
            inline: false,
            layer: 0,
            specificity: Specificity::default(),
            proximity: UNSCOPED,
            order: (0, 0, 0),
        };
        if p.tier == ANIMATION_TIER || p.tier == TRANSITION_TIER {
            // These origins form their own layer.
            return Some(min(p.tier));
        }
        if p.inline && self.declaration.important {
            return None;
        }
        let origin = match self.level {
            RevertLevel::UserAgent => Origin::UserAgent,
            RevertLevel::User => Origin::User,
            RevertLevel::Author => Origin::Author,
        };
        // As if nothing between the layer's important and normal levels
        // existed, including the normal level itself.
        Some(Priority {
            context: context_key(self.depth, false),
            inline: p.inline,
            layer: layer_key(self.rank, false),
            ..min(tier(origin, false))
        })
    }
}

/// Where a cascade's stylesheet belongs: its tree, and the layer tree
/// nodes of its declared layers.
#[derive(Clone, Debug, PartialEq)]
struct Placement {
    context: TreeContext,
    layer_nodes: Vec<usize>,
}

/// An ordered set of stylesheets from any origins and trees, evaluated
/// against one media environment.
///
/// User-agent and user sheets style every tree; author sheets style the
/// tree they belong to (and its shadow host, through `:host`).
#[derive(Clone, Debug, Default)]
pub struct Cascade {
    sheets: Vec<StyleSheet>,
    placements: Vec<Placement>,
    environment: MediaEnvironment,
    /// Per sheet, whether each of its media query lists matches.
    media_results: Vec<Vec<bool>>,
    /// Layer trees and their ranks, per origin and tree (derived from the
    /// sheets and placements).
    layers: HashMap<(Origin, TreeContext), (LayerOrder, Vec<u32>)>,
    /// Custom properties registered with `@property` (derived from the
    /// sheets; a later registration of a name wins).
    registry: RegistryRef,
    /// The document base URL, which `url()` values in `style` attributes
    /// resolve against.
    document_base_url: Option<Url>,
    /// The document's CSP for `style` attributes.
    inline_policy: Option<std::rc::Rc<dyn crate::InlineStylePolicy>>,
}

impl PartialEq for Cascade {
    fn eq(&self, other: &Self) -> bool {
        self.sheets == other.sheets
            && self.placements == other.placements
            && self.environment == other.environment
            && self.document_base_url == other.document_base_url
    }
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

    /// Set the document base URL that `style` attributes resolve `url()`
    /// values against (sheets carry their own, [`StyleSheet::base_url`]).
    pub fn set_document_base_url(&mut self, base: Option<Url>) {
        self.document_base_url = base;
    }

    /// Apply a Content Security Policy to `style` attributes.
    pub fn set_inline_policy(&mut self, policy: Option<std::rc::Rc<dyn crate::InlineStylePolicy>>) {
        self.inline_policy = policy;
    }

    /// Whether a `style` attribute with this value may apply.
    pub fn allows_style_attribute(&self, value: &str) -> bool {
        self.inline_policy
            .as_ref()
            .is_none_or(|policy| policy.allows_style_attribute(value))
    }

    /// The document base URL set with [`Cascade::set_document_base_url`].
    pub fn document_base_url(&self) -> Option<&Url> {
        self.document_base_url.as_ref()
    }

    /// Append a document-tree stylesheet; later sheets come later in order
    /// of appearance.
    pub fn push(&mut self, sheet: StyleSheet) {
        self.push_in(sheet, TreeContext::DOCUMENT);
    }

    /// Append a stylesheet belonging to the tree `context` (for user-agent
    /// and user sheets, which style every tree, `context` only scopes their
    /// layer order).
    pub fn push_in(&mut self, sheet: StyleSheet, context: TreeContext) {
        let index = self.sheets.len();
        let (order, ranks) = self.layers.entry((sheet.origin, context)).or_default();
        let layer_nodes = sheet
            .layers
            .iter()
            .map(|name| order.declare(index, name))
            .collect();
        *ranks = order.ranks();
        self.media_results.push(evaluate(&sheet, &self.environment));
        if !sheet.properties.is_empty() {
            let mut registry: Registry = self.registry.0.as_deref().cloned().unwrap_or_default();
            for registration in &sheet.properties {
                registry.insert(registration.name.clone(), registration.clone());
            }
            self.registry = RegistryRef(Some(std::sync::Arc::new(registry)));
        }
        self.sheets.push(sheet);
        self.placements.push(Placement {
            context,
            layer_nodes,
        });
    }

    /// The stylesheets in order.
    pub fn sheets(&self) -> &[StyleSheet] {
        &self.sheets
    }

    /// The custom property registration in effect for `name`, if any.
    pub fn registration(&self, name: &str) -> Option<&PropertyRegistration> {
        self.registry.0.as_deref().and_then(|r| r.get(name))
    }

    /// The `@keyframes` rule named `name` for an element in the tree
    /// `context` (CSS Animations 1 §3, CSS Scoping 1 §3.5): author rules
    /// of that tree, then of the trees around it, then user and user-agent
    /// rules; within one tree the rule in the highest-priority cascade
    /// layer wins, and among those the last one. Rules inside an `@media`
    /// that does not match are ignored. `outer` lists the contexts of the
    /// enclosing trees, innermost first.
    pub fn keyframes(
        &self,
        name: &str,
        context: TreeContext,
        outer: &[TreeContext],
    ) -> Option<&std::sync::Arc<crate::KeyframesRule>> {
        let contexts = std::iter::once(context).chain(outer.iter().copied());
        let origins = [Origin::Author, Origin::User, Origin::UserAgent];
        for (origin, context) in origins.iter().flat_map(|&o| {
            let list: Vec<TreeContext> = if o == Origin::Author {
                contexts.clone().collect()
            } else {
                vec![TreeContext::DOCUMENT]
            };
            list.into_iter().map(move |c| (o, c))
        }) {
            type Ranked<'r> = (
                (u32, usize, usize),
                &'r std::sync::Arc<crate::KeyframesRule>,
            );
            let mut best: Option<Ranked<'_>> = None;
            for (sheet_index, ((sheet, placement), media)) in self
                .sheets
                .iter()
                .zip(&self.placements)
                .zip(&self.media_results)
                .enumerate()
            {
                if sheet.origin != origin
                    || (origin == Origin::Author && placement.context != context)
                {
                    continue;
                }
                let ranks = self
                    .layers
                    .get(&(sheet.origin, placement.context))
                    .map_or(&[][..], |(_, ranks)| ranks);
                for (rule_index, rule) in sheet.keyframes.iter().enumerate() {
                    if &*rule.name != name
                        || !rule
                            .media
                            .iter()
                            .all(|&i| media.get(i).copied().unwrap_or(false))
                    {
                        continue;
                    }
                    let key = (
                        Self::rank(ranks, placement, rule.layer),
                        sheet_index,
                        rule_index,
                    );
                    if best.as_ref().is_none_or(|(b, _)| key > *b) {
                        best = Some((key, rule));
                    }
                }
            }
            if let Some((_, rule)) = best {
                return Some(rule);
            }
        }
        None
    }

    /// Whether any sheet has `@keyframes` rules.
    pub fn has_keyframes(&self) -> bool {
        self.sheets.iter().any(|s| !s.keyframes.is_empty())
    }

    /// Whether any sheet has `@container` rules.
    pub fn has_container_queries(&self) -> bool {
        self.sheets.iter().any(|s| !s.containers.is_empty())
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

    /// Compute the specified style of a document-tree `element`.
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
        let inputs = StyleInputs {
            style_attribute: inline,
            ..StyleInputs::document()
        };
        self.specified_style_with(element, cx, &inputs, parent)
    }

    /// [`Self::specified_style`] for an element in any tree, with animation
    /// and transition values.
    pub fn specified_style_with<E: ElementView>(
        &self,
        element: E,
        cx: &MatchContext<E>,
        inputs: &StyleInputs<'_, E>,
        parent: Option<&ComputedStyle>,
    ) -> SpecifiedStyle {
        self.specified_style_cached(element, cx, inputs, parent, &MatchCache::default())
    }

    /// The rank of layer `rule_layer` of a sheet placed at `placement`,
    /// given its layer group's `ranks`.
    fn rank(ranks: &[u32], placement: &Placement, rule_layer: Option<usize>) -> u32 {
        rule_layer
            .and_then(|l| placement.layer_nodes.get(l))
            .and_then(|&node| ranks.get(node))
            .copied()
            .unwrap_or(UNLAYERED)
    }

    fn candidates<'a, E: ElementView>(
        &'a self,
        element: E,
        cx: &MatchContext<E>,
        inputs: &StyleInputs<'a, E>,
        cache: &MatchCache<E>,
    ) -> Vec<Candidate<'a>> {
        let mut candidates: Vec<Candidate<'a>> = Vec::new();
        let sheets = self
            .sheets
            .iter()
            .zip(&self.placements)
            .zip(&self.media_results);
        for (sheet_index, ((sheet, placement), media)) in sheets.enumerate() {
            let author = sheet.origin == Origin::Author;
            // (host of the tree the rules match in, context depth)
            let tree = if !author || placement.context == inputs.scope.context {
                (inputs.scope.host, inputs.scope.depth)
            } else if inputs.shadow_root == Some(placement.context) {
                (Some(element), inputs.scope.depth.saturating_add(1))
            } else {
                continue;
            };
            let ranks = self
                .layers
                .get(&(sheet.origin, placement.context))
                .map_or(&[][..], |(_, ranks)| ranks);
            let level = match sheet.origin {
                Origin::UserAgent => RevertLevel::UserAgent,
                Origin::User => RevertLevel::User,
                Origin::Author => RevertLevel::Author,
            };
            for (rule_index, rule) in sheet.rules.iter().enumerate() {
                if !StyleSheet::rule_applies(rule, media) {
                    continue;
                }
                let matched = if let Some(scope) = &rule.scope {
                    scope.match_rule(&rule.selectors, element, cx, cache, tree.0)
                } else {
                    rule.selectors
                        .selectors()
                        .iter()
                        .filter(|s| {
                            s.pseudo_element().is_none()
                                && matches_in_tree(s, element, cx, cache, tree.0)
                        })
                        .map(|s| s.specificity())
                        .max()
                        .map(|specificity| (specificity, UNSCOPED))
                };
                let Some((specificity, proximity)) = matched else {
                    continue;
                };
                // Container conditions walk the ancestors and may substitute
                // `var()`, so test them only for rules whose selector matched
                // (TD-34); both must hold either way.
                if !sheet.containers.is_empty()
                    && !rule.media.iter().all(|&i| {
                        sheet.container_at(i).is_none_or(|list| {
                            container_list_matches(list, inputs.containers, &self.environment)
                        })
                    })
                {
                    continue;
                }
                let rank = Self::rank(ranks, placement, rule.layer);
                for (declaration_index, declaration) in rule.declarations.iter().enumerate() {
                    let important = declaration.important;
                    candidates.push(Candidate {
                        priority: Priority {
                            tier: tier(sheet.origin, important),
                            context: context_key(tree.1, important),
                            inline: false,
                            layer: layer_key(rank, important),
                            specificity,
                            proximity,
                            order: (sheet_index, rule_index, declaration_index),
                        },
                        level,
                        depth: tree.1,
                        rank,
                        declaration,
                    });
                }
            }
        }
        let depth = inputs.scope.depth;
        for (declaration_index, declaration) in inputs.style_attribute.iter().enumerate() {
            let important = declaration.important;
            candidates.push(Candidate {
                priority: Priority {
                    tier: tier(Origin::Author, important),
                    context: context_key(depth, important),
                    inline: true,
                    layer: layer_key(UNLAYERED, important),
                    specificity: Specificity::default(),
                    proximity: UNSCOPED,
                    order: (usize::MAX, 0, declaration_index),
                },
                level: RevertLevel::Author,
                depth,
                rank: UNLAYERED,
                declaration,
            });
        }
        for (tier, list) in [
            (ANIMATION_TIER, inputs.animations),
            (TRANSITION_TIER, inputs.transitions),
        ] {
            for (index, declaration) in list.iter().enumerate() {
                candidates.push(Candidate {
                    priority: Priority {
                        tier,
                        context: 0,
                        inline: false,
                        layer: 0,
                        specificity: Specificity::default(),
                        proximity: UNSCOPED,
                        order: (0, 0, index),
                    },
                    level: RevertLevel::Author,
                    depth,
                    rank: UNLAYERED,
                    declaration,
                });
            }
        }
        candidates.sort_by_key(|c| std::cmp::Reverse(c.priority));
        candidates
    }

    /// [`Self::specified_style_with`] sharing `cache` across one restyle.
    pub(crate) fn specified_style_cached<E: ElementView>(
        &self,
        element: E,
        cx: &MatchContext<E>,
        inputs: &StyleInputs<'_, E>,
        parent: Option<&ComputedStyle>,
        cache: &MatchCache<E>,
    ) -> SpecifiedStyle {
        let candidates = self.candidates(element, cx, inputs, cache);
        let initial;
        let is_root = parent.is_none();
        let parent = match parent {
            Some(parent) => parent,
            None => {
                initial = ComputedStyle::initial();
                &initial
            }
        };
        // Custom properties first: longhands may refer to them.
        let mut by_name: HashMap<&str, Vec<&Candidate<'_>>> = HashMap::new();
        let mut names: Vec<&str> = Vec::new();
        for candidate in &candidates {
            if let Some(name) = candidate.declaration.property.custom() {
                by_name
                    .entry(name)
                    .or_insert_with(|| {
                        names.push(name);
                        Vec::new()
                    })
                    .push(candidate);
            }
        }
        names.sort_unstable();
        let outcomes = names
            .iter()
            .map(|&name| {
                let outcome = match resolve(by_name[name].iter().copied()) {
                    Resolved::Declared(DeclaredValue::Custom(value)) => CustomOutcome::Value(value),
                    Resolved::Initial => CustomOutcome::Initial,
                    Resolved::Inherit => CustomOutcome::Inherit,
                    Resolved::Declared(_) | Resolved::Unset => CustomOutcome::Unset,
                };
                (name, outcome)
            })
            .collect();
        let unparsed = |property: PropertyId| match resolve(
            candidates
                .iter()
                .filter(|c| c.declaration.property == property),
        ) {
            Resolved::Declared(DeclaredValue::Unparsed(unparsed)) => Some(&**unparsed),
            _ => None,
        };
        let font = FontDependencies {
            font_size: unparsed(PropertyId::FontSize),
            line_height: unparsed(PropertyId::LineHeight),
            root: is_root,
        };
        let CustomResult {
            custom,
            font_size_cycle,
            line_height_cycle,
        } = cache.substitution.borrow_mut().custom_properties(
            &parent.custom,
            outcomes,
            &self.registry,
            font,
        );
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
                let in_cycle = match property {
                    PropertyId::FontSize => font_size_cycle,
                    PropertyId::LineHeight => line_height_cycle,
                    _ => false,
                };
                if in_cycle {
                    return unset();
                }
                let resolved = resolve(
                    candidates
                        .iter()
                        .filter(|c| c.declaration.property == property),
                );
                match resolved {
                    Resolved::Declared(DeclaredValue::Value(value)) => value.clone(),
                    Resolved::Declared(DeclaredValue::Unparsed(unparsed)) => cache
                        .substitution
                        .borrow_mut()
                        .longhand(unparsed, property, &custom)
                        .unwrap_or_else(unset),
                    Resolved::Initial => property.initial_value(),
                    Resolved::Inherit => inherit(),
                    Resolved::Declared(_) | Resolved::Unset => unset(),
                }
            })
            .collect();
        SpecifiedStyle { values, custom }
    }
}

/// The cascade's decision for one property.
enum Resolved<'a> {
    /// The winning declaration's value (never a CSS-wide keyword).
    Declared(&'a DeclaredValue),
    Initial,
    Inherit,
    Unset,
}

/// Pick the winner among one property's `candidates` (highest priority
/// first), resolving `revert` and `revert-layer` by skipping candidates.
fn resolve<'a, 'b: 'a>(candidates: impl Iterator<Item = &'a Candidate<'b>>) -> Resolved<'b> {
    // `revert`: origins at or above this level are skipped.
    let mut ceiling: Option<RevertLevel> = None;
    // `revert-layer`: candidates at or above this are skipped.
    let mut floor: Option<Priority> = None;
    // `revert-layer` in an important style attribute: style attribute and
    // animation declarations are skipped.
    let mut skip_attached = false;
    for candidate in candidates {
        if ceiling.is_some_and(|c| candidate.level >= c)
            || floor.is_some_and(|f| candidate.priority >= f)
            || (skip_attached
                && (candidate.priority.inline || candidate.priority.tier == ANIMATION_TIER))
        {
            continue;
        }
        match &candidate.declaration.value {
            DeclaredValue::CssWide(CssWideKeyword::Initial) => return Resolved::Initial,
            DeclaredValue::CssWide(CssWideKeyword::Inherit) => return Resolved::Inherit,
            DeclaredValue::CssWide(CssWideKeyword::Unset) => return Resolved::Unset,
            DeclaredValue::CssWide(CssWideKeyword::Revert) => {
                if candidate.level == RevertLevel::UserAgent {
                    return Resolved::Unset;
                }
                ceiling = Some(candidate.level);
            }
            DeclaredValue::CssWide(CssWideKeyword::RevertLayer) => {
                match candidate.revert_layer_floor() {
                    Some(f) => floor = Some(f),
                    None => skip_attached = true,
                }
            }
            value => return Resolved::Declared(value),
        }
    }
    Resolved::Unset
}
