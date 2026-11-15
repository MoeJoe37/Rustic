//! Explicit stylesheet scopes (CSS Cascade Level 6 §3.5).

use std::cmp::Reverse;

use rustic_css::{
    BlockKind, ComponentValue, NamespaceMap, SelectorList, Specificity, Token, Value,
    parse_scoped_selector_list, parse_selector_list,
};

use crate::matching::{MatchCache, matches_in_tree};
use crate::{ElementView, MatchContext};

/// The explicit root selector and optional limit of a stylesheet scope.
#[derive(Clone, Debug, PartialEq)]
pub struct ScopeRule {
    start: SelectorList,
    end: Option<SelectorList>,
}

impl ScopeRule {
    /// The selectors for elements that can anchor the scope.
    pub fn start(&self) -> &SelectorList {
        &self.start
    }

    /// The selectors that exclude subtrees of each root.
    pub fn end(&self) -> Option<&SelectorList> {
        self.end.as_ref()
    }

    pub(crate) fn parse(values: &[Value], namespaces: &NamespaceMap) -> Option<Self> {
        let mut parts = values.iter().filter(|value| !value.is_whitespace());
        let start = parse_selector_list(parentheses(parts.next()?)?, namespaces).ok()?;
        if start
            .selectors()
            .iter()
            .any(|s| s.pseudo_element().is_some())
        {
            return None;
        }
        let end = match parts.next() {
            None => None,
            Some(part) if matches!(part.token(), Some(Token::Ident(s)) if s.eq_ignore_ascii_case("to")) => {
                Some(
                    parse_scoped_selector_list(parentheses(parts.next()?)?, namespaces, false)
                        .ok()?,
                )
            }
            _ => return None,
        };
        parts.next().is_none().then_some(Self { start, end })
    }

    pub(crate) fn match_rule<E: ElementView>(
        &self,
        selectors: &SelectorList,
        element: E,
        cx: &MatchContext<E>,
        cache: &MatchCache<E>,
        host: Option<E>,
    ) -> Option<(Specificity, Reverse<usize>)> {
        // Each candidate root must contain the subject in its own tree. A
        // shadow host can itself be a root, but the walk never passes it.
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
        let mut best = None;
        let highest = selectors.max_specificity();
        for (hops, root) in ancestors().enumerate() {
            if !self
                .start
                .selectors()
                .iter()
                .any(|s| matches_in_tree(s, root, cx, cache, host))
            {
                continue;
            }
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
            if self.end.as_ref().is_some_and(|end| {
                ancestors().take(hops).any(|ancestor| {
                    end.selectors()
                        .iter()
                        .any(|s| matches_in_tree(s, ancestor, &scoped, &scoped_cache, host))
                })
            }) {
                continue;
            }
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
