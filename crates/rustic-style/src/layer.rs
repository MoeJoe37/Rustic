//! Cascade layers (CSS Cascade Level 5 §6.4): layer names and layer order.
//!
//! A stylesheet lists the layers it declares, in order of first appearance
//! ([`crate::StyleSheet::layers`]). [`LayerOrder`] merges those lists for one
//! origin and tree context, in sheet order, into a layer tree and ranks every
//! layer: sublayers sort before their parent's own rules, sibling layers sort
//! in declaration order, and unlayered rules form the implicit final layer.

use std::collections::HashMap;
use std::fmt;

/// One segment of a layer name.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum LayerSegment {
    /// A named layer, such as `framework` in `@layer framework`.
    Named(String),
    /// An anonymous layer (`@layer { ... }`), unique within its sheet: the
    /// number counts the sheet's anonymous layer rules in source order.
    Anonymous(u32),
}

/// A fully qualified layer name, outermost segment first.
///
/// `@layer a { @layer b.c { ... } }` puts its rules in layer `a.b.c`.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LayerName {
    segments: Vec<LayerSegment>,
}

impl LayerName {
    /// A name from its segments, outermost first; `None` if empty.
    pub fn new(segments: Vec<LayerSegment>) -> Option<Self> {
        (!segments.is_empty()).then_some(Self { segments })
    }

    /// The segments, outermost first (never empty).
    pub fn segments(&self) -> &[LayerSegment] {
        &self.segments
    }
}

impl fmt::Display for LayerName {
    /// Named segments joined with `.`; anonymous ones print as `<anonymous>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, segment) in self.segments.iter().enumerate() {
            if index > 0 {
                f.write_str(".")?;
            }
            match segment {
                LayerSegment::Named(name) => f.write_str(name)?,
                LayerSegment::Anonymous(_) => f.write_str("<anonymous>")?,
            }
        }
        Ok(())
    }
}

/// The rank of unlayered declarations: after every explicit layer.
pub(crate) const UNLAYERED: u32 = u32::MAX;

/// A child's key under its parent. Anonymous segments are qualified by the
/// index of the sheet that declared them, so equal anonymous numbers from
/// different sheets stay distinct layers.
type ChildKey = (usize, Option<usize>, LayerSegment);

/// Layer ranks for one origin and tree context.
#[derive(Clone, Debug)]
pub(crate) struct LayerOrder {
    /// Each node's children in declaration order; node 0 is the implicit
    /// root whose own rules are the unlayered ones.
    children: Vec<Vec<usize>>,
    lookup: HashMap<ChildKey, usize>,
}

impl Default for LayerOrder {
    fn default() -> Self {
        Self {
            children: vec![Vec::new()],
            lookup: HashMap::new(),
        }
    }
}

impl LayerOrder {
    /// Declare `name` (from sheet `sheet`), creating missing ancestors in
    /// order, and return its node.
    pub(crate) fn declare(&mut self, sheet: usize, name: &LayerName) -> usize {
        let mut node = 0;
        for segment in name.segments() {
            let owner = match segment {
                LayerSegment::Named(_) => None,
                LayerSegment::Anonymous(_) => Some(sheet),
            };
            let next = self.children.len();
            let child = *self
                .lookup
                .entry((node, owner, segment.clone()))
                .or_insert(next);
            if child == next {
                self.children.push(Vec::new());
                self.children[node].push(child);
            }
            node = child;
        }
        node
    }

    /// Every node's rank: a post-order numbering, so a layer ranks after its
    /// sublayers and the root (unlayered) ranks last, as [`UNLAYERED`].
    pub(crate) fn ranks(&self) -> Vec<u32> {
        let mut ranks = vec![UNLAYERED; self.children.len()];
        let mut next: u32 = 0;
        // Iterative post-order: (node, index of the next child to visit).
        let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
        while let Some(&mut (node, ref mut child)) = stack.last_mut() {
            if let Some(&c) = self.children[node].get(*child) {
                *child += 1;
                stack.push((c, 0));
            } else {
                stack.pop();
                if node != 0 {
                    ranks[node] = next;
                    // Fewer than u32::MAX layers exist: each needs a rule.
                    next = next.saturating_add(1).min(UNLAYERED - 1);
                }
            }
        }
        ranks
    }
}
