//! Stylesheet scopes (CSS Cascade Level 6 §3.5): explicit, implicit
//! (`@scope` without a start) and nested ones.

use std::cmp::Reverse;
use std::sync::Arc;

use rustic_css::{
    BlockKind, ComponentValue, NamespaceMap, SelectorList, Specificity, Token, Value,
    parse_nested_selector_list, parse_scoped_selector_list, parse_selector_list,
};
use rustic_dom::NodeId;

use crate::matching::{MatchCache, matches_in_tree};
use crate::{ElementView, MatchContext};

/// Where a stylesheet scope's roots come from.
#[derive(Clone, Debug, PartialEq)]
pub enum ScopeStart {
    /// `@scope (<scope-start>)`: elements matching the selectors (relative
    /// to the enclosing scope's roots when nested).
    Selectors(SelectorList),
    /// `@scope` without a start: the owner node's parent element (or the
    /// shadow host), set when the sheet is attached; `None` until then,
    /// which makes the tree's root element the root.
    Implicit(Option<NodeId>),
}

/// A stylesheet scope (CSS Cascade Level 6 §3.5): its roots, optional
/// limit and the enclosing scope it is nested in.
#[derive(Clone, Debug, PartialEq)]
pub struct ScopeRule {
    start: ScopeStart,
    end: Option<SelectorList>,
    parent: Option<Arc<ScopeRule>>,
}

impl ScopeRule {
    /// The selectors for elements that can anchor the scope, unless the
    /// scope is implicit.
    pub fn start(&self) -> Option<&SelectorList> {
        match &self.start {
            ScopeStart::Selectors(list) => Some(list),
            ScopeStart::Implicit(_) => None,
        }
    }

    /// Where the roots come from.
    pub fn start_kind(&self) -> &ScopeStart {
        &self.start
    }

    /// The selectors that exclude subtrees of each root.
    pub fn end(&self) -> Option<&SelectorList> {
        self.end.as_ref()
    }

    /// The enclosing scope, for a nested `@scope`.
    pub fn parent(&self) -> Option<&Arc<ScopeRule>> {
        self.parent.as_ref()
    }

    /// Parse an `@scope` prelude. A nested scope's start is relative to
    /// the enclosing scope's roots; `nesting` gives the selectors of an
    /// enclosing style rule, which an omitted start uses and `&` refers to.
    pub(crate) fn parse(
        values: &[Value],
        namespaces: &NamespaceMap,
        parent: Option<Arc<ScopeRule>>,
        nesting: Option<&SelectorList>,
    ) -> Option<Self> {
        let mut parts = values
            .iter()
            .filter(|value| !value.is_whitespace())
            .peekable();
        let is_to = |value: &&Value| matches!(value.token(), Some(Token::Ident(s)) if s.eq_ignore_ascii_case("to"));
        let start = match parts.peek() {
            None => None,
            Some(part) if is_to(part) => None,
            Some(_) => {
                let block = parentheses(parts.next()?)?;
                let list = match (nesting, &parent) {
                    (Some(nesting), _) => {
                        parse_nested_selector_list(block, namespaces, &Arc::new(nesting.clone()))
                    }
                    (None, Some(_)) => parse_scoped_selector_list(block, namespaces, false),
                    (None, None) => parse_selector_list(block, namespaces),
                }
                .ok()?;
                if list
                    .selectors()
                    .iter()
                    .any(|s| s.pseudo_element().is_some())
                {
                    return None;
                }
                Some(list)
            }
        };
        let start = match (start, nesting) {
            (Some(list), _) => ScopeStart::Selectors(list),
            // An omitted start in a style rule scopes to its subjects.
            (None, Some(nesting)) => ScopeStart::Selectors(nesting.clone()),
            (None, None) if parent.is_some() => return None,
            (None, None) => ScopeStart::Implicit(None),
        };
        let end = match parts.next() {
            None => None,
            Some(part) if is_to(&part) => Some(
                parse_scoped_selector_list(parentheses(parts.next()?)?, namespaces, false).ok()?,
            ),
            _ => return None,
        };
        parts
            .next()
            .is_none()
            .then_some(Self { start, end, parent })
    }

    /// This scope with its implicit roots (and those of enclosing scopes)
    /// set to `owner`'s.
    pub(crate) fn with_owner(&self, owner: Option<NodeId>) -> Self {
        Self {
            start: match &self.start {
                ScopeStart::Implicit(_) => ScopeStart::Implicit(owner),
                other => other.clone(),
            },
            end: self.end.clone(),
            parent: self.parent.as_ref().map(|p| Arc::new(p.with_owner(owner))),
        }
    }

    /// This scope nested in `outer` (an `@import ... scope()` wrapping a
    /// sheet's scopes): the outermost scope gets `outer` as its parent.
    pub(crate) fn within(&self, outer: &Arc<ScopeRule>) -> Self {
        Self {
            start: self.start.clone(),
            end: self.end.clone(),
            parent: Some(match &self.parent {
                Some(parent) => Arc::new(parent.within(outer)),
                None => Arc::clone(outer),
            }),
        }
    }

    /// The scoping roots of `element`: its inclusive ancestors (in its
    /// tree) that are roots of this scope and whose limits do not exclude
    /// it, with their distance in generations.
    fn roots<E: ElementView>(
        &self,
        element: E,
        cx: &MatchContext<E>,
        cache: &MatchCache<E>,
        host: Option<E>,
    ) -> Vec<(E, usize)> {
        let ancestors = || {
            std::iter::successors(Some(element), |ancestor| {
                if Some(*ancestor) == host {
                    return None;
                }
                ancestor.parent_element().or_else(|| {
                    ancestor
                        .parent_shadow_host()
                        .filter(|parent| Some(*parent) == host)
                })
            })
        };
        // The element must be in the enclosing scope too.
        let outer_of_element = self
            .parent
            .as_ref()
            .map(|parent| parent.roots(element, cx, cache, host));
        if outer_of_element.as_ref().is_some_and(Vec::is_empty) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (hops, root) in ancestors().enumerate() {
            let is_root = match &self.start {
                ScopeStart::Implicit(owner) => match owner {
                    Some(owner) => root.node_id() == Some(*owner),
                    None => root.is_root() || Some(root) == host,
                },
                ScopeStart::Selectors(list) => match &outer_of_element {
                    None => list
                        .selectors()
                        .iter()
                        .any(|s| matches_in_tree(s, root, cx, cache, host)),
                    Some(outer) => outer.iter().any(|&(outer_root, _)| {
                        let scoped = MatchContext {
                            scope: Some(outer_root),
                            ..*cx
                        };
                        let scoped_cache = MatchCache::default();
                        list.selectors()
                            .iter()
                            .any(|s| matches_in_tree(s, root, &scoped, &scoped_cache, host))
                    }),
                },
            };
            if !is_root {
                continue;
            }
            if let Some(end) = &self.end {
                let scoped = MatchContext {
                    scope: Some(root),
                    ..*cx
                };
                let scoped_cache = MatchCache::default();
                if ancestors().take(hops).any(|ancestor| {
                    end.selectors()
                        .iter()
                        .any(|s| matches_in_tree(s, ancestor, &scoped, &scoped_cache, host))
                }) {
                    continue;
                }
            }
            out.push((root, hops));
        }
        out
    }

    pub(crate) fn match_rule<E: ElementView>(
        &self,
        selectors: &SelectorList,
        element: E,
        cx: &MatchContext<E>,
        cache: &MatchCache<E>,
        host: Option<E>,
    ) -> Option<(Specificity, Reverse<usize>)> {
        let mut best = None;
        let highest = selectors.max_specificity();
        for (root, hops) in self.roots(element, cx, cache, host) {
            let scoped = MatchContext {
                scope: Some(root),
                ..*cx
            };
            // Nested-selector results (including :has and filtered nth
            // positions) depend on :scope. Keep them local to this root.
            let scoped_cache = MatchCache::default();
            let specificity = selectors
                .selectors()
                .iter()
                .filter(|s| {
                    s.pseudo_element().is_none()
                        && matches_in_tree(s, element, &scoped, &scoped_cache, host)
                })
                .map(|s| s.specificity())
                .max();
            let Some(specificity) = specificity else {
                continue;
            };
            best = best.max(Some((specificity, Reverse(hops))));
            if specificity == highest {
                break;
            }
        }
        best
    }
}

fn parentheses(value: &Value) -> Option<&[Value]> {
    match &value.value {
        ComponentValue::Block {
            kind: BlockKind::Paren,
            contents,
        } => Some(contents),
        _ => None,
    }
}
