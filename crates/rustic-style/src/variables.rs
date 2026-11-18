//! Computed custom properties and `var()` resolution for one element.
//!
//! Custom properties inherit. An element's own custom property
//! declarations are resolved in dependency order; every custom property in
//! a dependency cycle (including references in fallbacks) is
//! guaranteed-invalid, as is one whose substitution fails. Then each
//! longhand that was declared with `var()` is substituted and parsed; a
//! failure makes it invalid at computed-value time, so it acts as `unset`.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;

use rustic_css::{
    CssParser, CustomValue, LengthPercentage, LengthUnit, MAX_SUBSTITUTED_VALUES, PropertyId,
    PropertySyntax, PropertyValue, RegisteredItem, UnparsedValue,
};

/// Longest custom property value a style dump prints in full.
const MAX_DUMPED_VALUE_BYTES: usize = 256;

/// The computed custom properties of an element: name to value, with
/// guaranteed-invalid properties absent. Cloning is cheap.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CustomProperties {
    map: Arc<BTreeMap<Arc<str>, CustomValue>>,
    registry: RegistryRef,
}

impl CustomProperties {
    pub(crate) fn inherit_without_declarations(&self) -> Self {
        compute_pass(self, &[], &self.registry, &[])
    }

    /// The value of custom property `name` (with its leading `--`).
    pub fn get(&self, name: &str) -> Option<&CustomValue> {
        self.map.get(name)
    }

    /// Number of custom properties with a value.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether no custom property has a value.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Whether both share one storage (as elements whose custom properties
    /// were computed from the same parent values and declarations do).
    pub fn shares_storage_with(&self, other: &CustomProperties) -> bool {
        Arc::ptr_eq(&self.map, &other.map)
    }

    /// An identity for cache keys, valid while `self` is alive.
    pub(crate) fn identity(&self) -> usize {
        Arc::as_ptr(&self.map) as usize
    }

    /// Custom properties in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &CustomValue)> {
        self.map.iter().map(|(k, v)| (&**k, v))
    }

    /// `name: value` lines in name order, for the properties whose value
    /// differs from `other`'s. Values longer than
    /// [`MAX_DUMPED_VALUE_BYTES`] are cut and end in `...`.
    pub(crate) fn dump_differences(&self, other: &CustomProperties, indent: &str) -> String {
        let mut out = String::new();
        for (name, value) in self.iter() {
            if other.get(name) != Some(value) {
                out.push_str(indent);
                let mut text = Bounded(String::new());
                if fmt::write(&mut text, format_args!("{value}")).is_err() {
                    text.0.push_str("...");
                }
                let text = text.0;
                let _ = fmt::write(&mut out, format_args!("{}: {text}\n", Name(name)));
            }
        }
        out
    }
}

/// A string that refuses to grow past [`MAX_DUMPED_VALUE_BYTES`], so long
/// values stop serializing early.
struct Bounded(String);

impl fmt::Write for Bounded {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let room = MAX_DUMPED_VALUE_BYTES.saturating_sub(self.0.len());
        if s.len() <= room {
            self.0.push_str(s);
            return Ok(());
        }
        let mut cut = room;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        self.0.push_str(&s[..cut]);
        Err(fmt::Error)
    }
}

/// A custom property name, serialized as an identifier.
struct Name<'a>(&'a str);

impl fmt::Display for Name<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        rustic_css::serialize_identifier(f, self.0)
    }
}

/// What the cascade decided for one custom property of an element.
#[derive(Clone, Copy)]
pub(crate) enum CustomOutcome<'a> {
    /// The declared value.
    Value(&'a CustomValue),
    /// `initial`.
    Initial,
    /// `inherit`.
    Inherit,
    /// `unset` (or `revert` to nothing).
    Unset,
}

impl CustomOutcome<'_> {
    fn key(&self) -> usize {
        match self {
            Self::Value(value) => *value as *const CustomValue as usize,
            Self::Initial => 1,
            Self::Inherit => 2,
            Self::Unset => 3,
        }
    }
}

/// A custom property registered with `@property`.
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyRegistration {
    /// The property name, with its leading `--`.
    pub name: String,
    /// The syntax its values must match.
    pub syntax: PropertySyntax,
    /// Whether it inherits.
    pub inherits: bool,
    /// Its initial value, already computed; `None` (guaranteed-invalid)
    /// only for the universal syntax.
    pub initial: Option<CustomValue>,
}

/// Registered custom properties by name.
pub(crate) type Registry = HashMap<String, PropertyRegistration>;

/// A shared registry (one per cascade), compared by identity first.
#[derive(Clone, Debug, Default)]
pub(crate) struct RegistryRef(pub(crate) Option<Arc<Registry>>);

impl PartialEq for RegistryRef {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
            (None, None) => true,
            _ => false,
        }
    }
}

/// What `font-size` and `line-height` refer to through `var()`, for
/// cycles through font-relative units in registered properties.
#[derive(Clone, Copy, Default)]
pub(crate) struct FontDependencies<'a> {
    pub font_size: Option<&'a UnparsedValue>,
    pub line_height: Option<&'a UnparsedValue>,
    /// The element is the root (root-relative units count too).
    pub root: bool,
}

/// Computed custom properties, and whether `font-size` and `line-height`
/// are in a dependency cycle (and so act as `unset`).
#[derive(Clone)]
pub(crate) struct CustomResult {
    pub custom: CustomProperties,
    pub font_size_cycle: bool,
    pub line_height_cycle: bool,
}

/// Parent identity, font dependencies and (name, outcome) pairs.
type CustomKey = (usize, usize, usize, bool, Vec<(String, usize)>);

/// Results shared across the elements of one restyle (TD-33): elements
/// with the same parent custom properties and the same winning custom
/// declarations share one computed map, and a `var()` longhand is
/// substituted once per (declaration, longhand, custom properties).
/// Every result is a function of its key alone (each computation gets a
/// fresh substitution budget), so caching never changes a style.
#[derive(Default)]
pub(crate) struct SubstitutionCache {
    /// Key to (parent, kept alive so its identity stays unique; result).
    custom: HashMap<CustomKey, (CustomProperties, CustomResult)>,
    /// (declaration address, longhand, custom properties identity) to
    /// (those custom properties, kept alive; result).
    longhands: HashMap<(usize, PropertyId, usize), (CustomProperties, Option<PropertyValue>)>,
}

impl SubstitutionCache {
    /// [`compute_custom_properties`], shared between equal inputs. The
    /// declared values must outlive the cache (they live in the cascade).
    pub(crate) fn custom_properties<'a>(
        &mut self,
        parent: &CustomProperties,
        outcomes: Vec<(&'a str, CustomOutcome<'a>)>,
        registry: &RegistryRef,
        font: FontDependencies<'_>,
    ) -> CustomResult {
        if outcomes.is_empty() && registry.0.is_none() && parent.registry == *registry {
            return CustomResult {
                custom: parent.clone(),
                font_size_cycle: false,
                line_height_cycle: false,
            };
        }
        let address =
            |u: Option<&UnparsedValue>| u.map_or(0, |u| u as *const UnparsedValue as usize);
        let key = (
            parent.identity(),
            address(font.font_size),
            address(font.line_height),
            font.root,
            outcomes
                .iter()
                .map(|(name, outcome)| ((*name).to_owned(), outcome.key()))
                .collect(),
        );
        if let Some((_, result)) = self.custom.get(&key) {
            return result.clone();
        }
        let result = compute_custom_properties(parent, &outcomes, registry, font);
        self.custom.insert(key, (parent.clone(), result.clone()));
        result
    }

    /// Substitute and parse `unparsed` as `property` against `custom`,
    /// shared between equal inputs. `unparsed` must outlive the cache.
    pub(crate) fn longhand(
        &mut self,
        unparsed: &UnparsedValue,
        property: PropertyId,
        custom: &CustomProperties,
    ) -> Option<PropertyValue> {
        let key = (
            unparsed as *const UnparsedValue as usize,
            property,
            custom.identity(),
        );
        if let Some((_, result)) = self.longhands.get(&key) {
            return result.clone();
        }
        let mut budget = MAX_SUBSTITUTED_VALUES;
        let lookup = |name: &str| custom.get(name);
        let mut result = unparsed.resolve(property, &lookup, &mut budget);
        if let (Some(value), Some(base)) = (result.as_mut(), unparsed.url_base()) {
            if value.has_urls() {
                if let Ok(base) = rustic_url::Url::parse(base, None) {
                    value.resolve_urls(&|url| crate::sheet::resolve_css_url(url, &base));
                }
            }
        }
        self.longhands.insert(key, (custom.clone(), result.clone()));
        result
    }
}

/// A computed value from its canonical serialization.
fn reparse(text: &str) -> Option<CustomValue> {
    let values = CssParser::default().component_values(text).ok()?.value;
    CustomValue::new(&values)
}

/// `value` checked against `name`'s registration and put in canonical
/// form; `None` if it does not match the syntax. Unregistered and `*`
/// values pass unchanged.
fn validate(registry: Option<&Registry>, name: &str, value: &CustomValue) -> Option<CustomValue> {
    let Some(registration) = registry.and_then(|r| r.get(name)) else {
        return Some(value.clone());
    };
    if registration.syntax == PropertySyntax::Universal {
        return Some(value.clone());
    }
    let parsed = registration.syntax.parse_value(value)?;
    let canonical = parsed.to_string();
    if canonical == value.to_string() {
        Some(value.clone())
    } else {
        reparse(&canonical)
    }
}

/// One pass of [`compute_custom_properties`], treating `forced` names as
/// `unset`. Returns the map and the element's own declared values after
/// substitution (for the font cycle check).
fn compute_pass<'a>(
    parent: &CustomProperties,
    outcomes: &[(&'a str, CustomOutcome<'a>)],
    registry: &RegistryRef,
    forced: &[&str],
) -> CustomProperties {
    let reg = registry.0.as_deref();
    let registration = |name: &str| reg.and_then(|r| r.get(name));
    let mut result = parent.clone();
    result.registry = registry.clone();
    // Registered properties that do not inherit start from their initial
    // value, and inherited ones without a value (at the root) too.
    let mut resets: Vec<(&str, Option<&CustomValue>)> = Vec::new();
    if let Some(reg) = reg {
        for (name, r) in reg {
            let current = parent.get(name);
            if (!r.inherits || current.is_none()) && current != r.initial.as_ref() {
                resets.push((name, r.initial.as_ref()));
            }
        }
        resets.sort_unstable_by_key(|(name, _)| *name);
    }
    if outcomes.is_empty() && resets.is_empty() {
        return result;
    }
    let map = Arc::make_mut(&mut result.map);
    let set =
        |map: &mut BTreeMap<Arc<str>, CustomValue>, name: &str, value: Option<CustomValue>| {
            match value {
                Some(value) => map.insert(name.into(), value),
                None => map.remove(name),
            };
        };
    for (name, initial) in resets {
        set(map, name, initial.cloned());
    }
    // `unset`: the parent's value if the property inherits, else initial.
    let unset = |name: &str| match registration(name) {
        Some(r) if !r.inherits => r.initial.clone(),
        Some(r) => parent.get(name).cloned().or_else(|| r.initial.clone()),
        None => parent.get(name).cloned(),
    };
    // Declared values with references, resolved below in dependency order.
    let mut pending: Vec<(&'a str, &'a CustomValue)> = Vec::new();
    for &(name, outcome) in outcomes {
        let outcome = if forced.contains(&name) {
            CustomOutcome::Unset
        } else {
            outcome
        };
        match outcome {
            CustomOutcome::Value(value) if value.has_references() => {
                map.remove(name);
                pending.push((name, value));
            }
            CustomOutcome::Value(value) => {
                let valid = validate(reg, name, value);
                let value = if valid.is_some() { valid } else { unset(name) };
                set(map, name, value);
            }
            CustomOutcome::Initial => {
                set(
                    map,
                    name,
                    registration(name).and_then(|r| r.initial.clone()),
                );
            }
            CustomOutcome::Inherit => set(map, name, parent.get(name).cloned()),
            CustomOutcome::Unset => set(map, name, unset(name)),
        }
    }
    if pending.is_empty() {
        return result;
    }
    let index: HashMap<&str, usize> = pending
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (*name, i))
        .collect();
    let edges: Vec<Vec<usize>> = pending
        .iter()
        .map(|(_, value)| {
            value
                .references()
                .into_iter()
                .filter_map(|name| index.get(name).copied())
                .collect()
        })
        .collect();
    for component in strongly_connected_components(&edges) {
        let cyclic = component.len() > 1 || edges[component[0]].contains(&component[0]);
        if cyclic {
            // Unregistered members become guaranteed-invalid; registered
            // ones act as `unset`.
            for &member in &component {
                let name = pending[member].0;
                if registration(name).is_some() {
                    set(map, name, unset(name));
                }
            }
            continue;
        }
        let (name, value) = pending[component[0]];
        let mut budget = MAX_SUBSTITUTED_VALUES;
        let lookup = |n: &str| map.get(n);
        let substituted = value
            .substitute(&lookup, &mut budget)
            .and_then(|v| validate(reg, name, &v));
        let value = if substituted.is_some() {
            substituted
        } else {
            unset(name)
        };
        set(map, name, value);
    }
    result
}

/// Compute an element's custom properties from its parent's, the cascade's
/// `outcomes` and the registry, then break cycles through font-relative
/// units: a registered length using `em` (and friends) depends on
/// `font-size`, one using `lh` also on `line-height`, and those depend on
/// what they refer to with `var()`. Every member of such a cycle acts as
/// `unset`.
pub(crate) fn compute_custom_properties<'a>(
    parent: &CustomProperties,
    outcomes: &[(&'a str, CustomOutcome<'a>)],
    registry: &RegistryRef,
    font: FontDependencies<'_>,
) -> CustomResult {
    let first = compute_pass(parent, outcomes, registry, &[]);
    let no_cycle = CustomResult {
        custom: first.clone(),
        font_size_cycle: false,
        line_height_cycle: false,
    };
    let Some(reg) = registry.0.as_deref() else {
        return no_cycle;
    };
    if font.font_size.is_none() && font.line_height.is_none() {
        return no_cycle;
    }
    // Nodes: the element's own declared custom properties, then font-size
    // and line-height.
    let names: Vec<&str> = outcomes.iter().map(|(name, _)| *name).collect();
    let index: HashMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let (font_node, line_node) = (names.len(), names.len() + 1);
    let refs = |u: Option<&UnparsedValue>| -> Vec<usize> {
        u.map(|u| {
            u.value()
                .references()
                .into_iter()
                .filter_map(|n| index.get(n).copied())
                .collect()
        })
        .unwrap_or_default()
    };
    let mut edges: Vec<Vec<usize>> = outcomes
        .iter()
        .map(|(name, outcome)| {
            let mut out: Vec<usize> = match outcome {
                CustomOutcome::Value(value) => value
                    .references()
                    .into_iter()
                    .filter_map(|n| index.get(n).copied())
                    .collect(),
                _ => Vec::new(),
            };
            let computed = reg
                .get(*name)
                .zip(first.get(name))
                .and_then(|(r, v)| r.syntax.parse_value(v));
            if let Some(value) = computed {
                let (on_font, on_line) = value.font_relative(font.root);
                if on_font {
                    out.push(font_node);
                }
                if on_line {
                    out.push(line_node);
                }
            }
            out
        })
        .collect();
    edges.push(refs(font.font_size));
    edges.push(refs(font.line_height));
    let mut forced: Vec<&str> = Vec::new();
    let (mut font_size_cycle, mut line_height_cycle) = (false, false);
    for component in strongly_connected_components(&edges) {
        if component.len() < 2 {
            continue;
        }
        if component.contains(&font_node) {
            font_size_cycle = true;
        }
        if component.contains(&line_node) {
            line_height_cycle = true;
        }
        if component.contains(&font_node) || component.contains(&line_node) {
            forced.extend(component.iter().filter_map(|&m| names.get(m).copied()));
        }
    }
    if forced.is_empty() {
        return no_cycle;
    }
    CustomResult {
        custom: compute_pass(parent, outcomes, registry, &forced),
        font_size_cycle,
        line_height_cycle,
    }
}

impl CustomProperties {
    /// Computed values for registered lengths: `compute` turns each into
    /// pixels (percentages stay). Unchanged storage when nothing needs it.
    pub(crate) fn absolutize(
        &self,
        compute: &dyn Fn(&LengthPercentage) -> LengthPercentage,
    ) -> CustomProperties {
        let Some(reg) = self.registry.0.as_deref() else {
            return self.clone();
        };
        let mut changes: Vec<(&str, Option<CustomValue>)> = Vec::new();
        for (name, value) in self.iter() {
            let Some(registration) = reg.get(name) else {
                continue;
            };
            let Some(mut parsed) = registration.syntax.parse_value(value) else {
                continue;
            };
            let needs = parsed.items.iter().any(|item| {
                matches!(item, RegisteredItem::Length(lp)
                    if !matches!(lp, LengthPercentage::Length(l) if l.unit == LengthUnit::Px)
                        && !matches!(lp, LengthPercentage::Percentage(_)))
            });
            if needs {
                parsed.map_lengths(compute);
                changes.push((name, reparse(&parsed.to_string())));
            }
        }
        if changes.is_empty() {
            return self.clone();
        }
        let mut result = self.clone();
        let map = Arc::make_mut(&mut result.map);
        for (name, value) in changes {
            match value {
                Some(value) => map.insert(name.into(), value),
                None => map.remove(name),
            };
        }
        result
    }
}

/// Tarjan's algorithm without recursion: the strongly connected components
/// of the graph, each listed after every component it has edges to.
fn strongly_connected_components(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    const UNVISITED: usize = usize::MAX;
    let count = edges.len();
    let mut order = vec![UNVISITED; count];
    let mut low = vec![0; count];
    let mut on_stack = vec![false; count];
    let mut stack: Vec<usize> = Vec::new();
    let mut components = Vec::new();
    let mut next = 0;
    // (node, index of the next edge to follow)
    let mut calls: Vec<(usize, usize)> = Vec::new();
    for start in 0..count {
        if order[start] != UNVISITED {
            continue;
        }
        calls.push((start, 0));
        order[start] = next;
        low[start] = next;
        next += 1;
        stack.push(start);
        on_stack[start] = true;
        while let Some(&mut (node, ref mut edge)) = calls.last_mut() {
            if let Some(&target) = edges[node].get(*edge) {
                *edge += 1;
                if order[target] == UNVISITED {
                    order[target] = next;
                    low[target] = next;
                    next += 1;
                    stack.push(target);
                    on_stack[target] = true;
                    calls.push((target, 0));
                } else if on_stack[target] {
                    low[node] = low[node].min(order[target]);
                }
                continue;
            }
            calls.pop();
            if let Some(&(caller, _)) = calls.last() {
                low[caller] = low[caller].min(low[node]);
            }
            if low[node] == order[node] {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                components.push(component);
            }
        }
    }
    components
}

#[cfg(test)]
mod tests {
    use super::strongly_connected_components;

    #[test]
    fn components_come_after_their_dependencies() {
        // 0 -> 1 -> 2 -> 1, 3 -> 0, 4 alone
        let edges = vec![vec![1], vec![2], vec![1], vec![0], vec![]];
        let components = strongly_connected_components(&edges);
        let position = |n: usize| components.iter().position(|c| c.contains(&n)).unwrap();
        assert_eq!(components.len(), 4);
        assert_eq!(position(1), position(2));
        assert!(position(1) < position(0));
        assert!(position(0) < position(3));
    }

    #[test]
    fn long_chains_do_not_recurse() {
        let edges: Vec<Vec<usize>> = (0..200_000).map(|i| vec![(i + 1) % 200_000]).collect();
        let components = strongly_connected_components(&edges);
        assert_eq!(components.len(), 1);
    }
}
