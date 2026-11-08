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

use rustic_css::CustomValue;

/// The computed custom properties of an element: name to value, with
/// guaranteed-invalid properties absent. Cloning is cheap.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CustomProperties {
    map: Arc<BTreeMap<Arc<str>, CustomValue>>,
}

impl CustomProperties {
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

    /// Custom properties in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &CustomValue)> {
        self.map.iter().map(|(k, v)| (&**k, v))
    }

    /// `name: value` lines in name order, for the properties whose value
    /// differs from `other`'s.
    pub(crate) fn dump_differences(&self, other: &CustomProperties, indent: &str) -> String {
        let mut out = String::new();
        for (name, value) in self.iter() {
            if other.get(name) != Some(value) {
                out.push_str(indent);
                let _ = fmt::write(&mut out, format_args!("{}: {value}\n", Name(name)));
            }
        }
        out
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
pub(crate) enum CustomOutcome<'a> {
    /// The declared value.
    Value(&'a CustomValue),
    /// `initial`: guaranteed-invalid.
    Initial,
    /// `inherit`, `unset` or no declaration: the parent's value.
    Inherit,
}

/// Compute an element's custom properties from its parent's and the
/// cascade's `outcomes`, charging substitution to `budget`.
pub(crate) fn compute_custom_properties<'a>(
    parent: &CustomProperties,
    outcomes: Vec<(&'a str, CustomOutcome<'a>)>,
    budget: &mut usize,
) -> CustomProperties {
    if outcomes
        .iter()
        .all(|(_, o)| matches!(o, CustomOutcome::Inherit))
    {
        return parent.clone();
    }
    let mut result = parent.clone();
    let map = Arc::make_mut(&mut result.map);
    // Declared values with references, resolved below in dependency order.
    let mut pending: Vec<(&'a str, &'a CustomValue)> = Vec::new();
    for (name, outcome) in outcomes {
        match outcome {
            CustomOutcome::Value(value) if value.has_references() => {
                map.remove(name);
                pending.push((name, value));
            }
            CustomOutcome::Value(value) => {
                map.insert(name.into(), value.clone());
            }
            CustomOutcome::Initial => {
                map.remove(name);
            }
            CustomOutcome::Inherit => {}
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
            // Members stay absent: guaranteed-invalid.
            continue;
        }
        let (name, value) = pending[component[0]];
        let lookup = |n: &str| map.get(n);
        let substituted = value.substitute(&lookup, budget);
        if let Some(substituted) = substituted {
            map.insert(name.into(), substituted);
        }
    }
    result
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
