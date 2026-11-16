//! Right-to-left selector matching (Selectors Level 4 §18).

use rustic_css::{
    AttributeCase, AttributeOperator, AttributeSelector, Combinator, ComplexSelector, Compound,
    NamespaceSelector, Nth, NthKind, PseudoClass, PseudoElement, RelativeSelector,
    RelativeSelectorList, SelectorList, SimpleSelector,
};
use rustic_dom::HTML_NAMESPACE;
use rustic_text::unicode::StrongDirection;

use crate::variables::SubstitutionCache;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ElementView;
use crate::html_state::{self, Descendants, TreeFacts};
use crate::text;

/// Document facts that change how selectors match.
#[derive(Clone, Copy, Debug)]
pub struct MatchContext<E> {
    /// An HTML document: type and attribute names match HTML elements ASCII
    /// case-insensitively, and HTML's legacy attributes compare values
    /// case-insensitively.
    pub html_document: bool,
    /// Quirks mode: ID and class selectors match ASCII case-insensitively.
    pub quirks: bool,
    /// The `:scope` element; `None` makes `:scope` match `:root`.
    pub scope: Option<E>,
}

impl<E> MatchContext<E> {
    /// A no-quirks HTML document with no scoping root.
    pub fn html() -> Self {
        Self {
            html_document: true,
            quirks: false,
            scope: None,
        }
    }

    /// A non-HTML (XML) document with no scoping root.
    pub fn xml() -> Self {
        Self {
            html_document: false,
            quirks: false,
            scope: None,
        }
    }
}

/// Whether any selector in `list` without a pseudo-element matches `element`.
///
/// Selectors that target a pseudo-element never match an element itself, as
/// in `Element.matches()`.
pub fn matches<E: ElementView>(list: &SelectorList, element: E, cx: &MatchContext<E>) -> bool {
    let cache = MatchCache::default();
    let m = Matcher {
        cx,
        cache: &cache,
        host: None,
        anchor: None,
    };
    list.selectors()
        .iter()
        .any(|s| s.pseudo_element().is_none() && complex(s, element, &m))
}

/// Whether `element` is the subject (originating element, for a selector
/// with a pseudo-element) of `selector`.
pub fn matches_complex<E: ElementView>(
    selector: &ComplexSelector,
    element: E,
    cx: &MatchContext<E>,
) -> bool {
    matches_complex_cached(selector, element, cx, &MatchCache::default())
}

/// [`matches_complex`] sharing `cache` with other matches against the same,
/// unchanged tree and the same, still-borrowed selectors (one restyle).
pub(crate) fn matches_complex_cached<E: ElementView>(
    selector: &ComplexSelector,
    element: E,
    cx: &MatchContext<E>,
    cache: &MatchCache<E>,
) -> bool {
    matches_in_tree(selector, element, cx, cache, None)
}

/// [`matches_complex_cached`] for a selector from a stylesheet of the shadow
/// tree hosted by `host` (`None` for the document tree).
///
/// Inside a shadow tree the host is *featureless*: it can match `:host` or
/// `:host()`, plus the scope selectors if it is the scoping root. Combinators
/// reach it from the shadow tree's top-level elements and stop there.
pub(crate) fn matches_in_tree<E: ElementView>(
    selector: &ComplexSelector,
    element: E,
    cx: &MatchContext<E>,
    cache: &MatchCache<E>,
    host: Option<E>,
) -> bool {
    complex(
        selector,
        element,
        &Matcher {
            cx,
            cache,
            host,
            anchor: None,
        },
    )
}

/// Which sibling count an `:nth-*` position uses, plus the address of its
/// `of S` list (0 for none).
type NthKey = (u8, usize);

/// Results reused across matches while the tree and the selectors stay
/// unchanged: nested selector-list results (`:is()`, `:where()`, `:not()`,
/// `:nth-*(of S)`) per element, and `:nth-*` sibling positions.
///
/// Without the list memo, an outer descendant combinator re-evaluates a
/// nested list from scratch for every ancestor it tries, which is exponential
/// in the nesting depth (TD-21). Without the position memo, matching every
/// child of an *n*-child parent against `:nth-child()` costs O(*n*²) (TD-20).
/// Keys use selector-list addresses, so a cache must not outlive the
/// selectors it was filled from: callers keep it for one match or restyle.
///
/// It also holds `:has()` subtree results per (compound, element), each
/// element's directionality, and whole-tree facts (IDs, radio groups,
/// default buttons, the pragma-set language) built on first use.
pub(crate) struct MatchCache<E> {
    lists: RefCell<HashMap<(usize, E), bool>>,
    positions: RefCell<HashMap<(NthKey, E), u64>>,
    subtrees: RefCell<HashMap<(usize, E), bool>>,
    pub(crate) directions: RefCell<HashMap<E, StrongDirection>>,
    pub(crate) tree: RefCell<Option<Rc<TreeFacts<E>>>>,
    /// Shared custom property and `var()` results (TD-33).
    pub(crate) substitution: RefCell<SubstitutionCache>,
}

impl<E> Default for MatchCache<E> {
    fn default() -> Self {
        Self {
            lists: RefCell::new(HashMap::new()),
            positions: RefCell::new(HashMap::new()),
            subtrees: RefCell::new(HashMap::new()),
            directions: RefCell::new(HashMap::new()),
            tree: RefCell::new(None),
            substitution: RefCell::new(SubstitutionCache::default()),
        }
    }
}

/// One match: the context plus the cache it shares, the shadow host when
/// matching a shadow tree's selector, and the `:has()` anchor while a
/// relative selector is being matched.
struct Matcher<'c, E> {
    cx: &'c MatchContext<E>,
    cache: &'c MatchCache<E>,
    host: Option<E>,
    anchor: Option<E>,
}

impl<E: ElementView> Matcher<'_, E> {
    /// Whether any selector of a nested `list` (pseudo-elements excluded by
    /// the parser) matches `e`, memoized by the list's address.
    fn list(&self, list: &SelectorList, e: E) -> bool {
        let key = (std::ptr::from_ref(list) as usize, e);
        if let Some(&known) = self.cache.lists.borrow().get(&key) {
            return known;
        }
        let result = list.selectors().iter().any(|s| complex(s, e, self));
        self.cache.lists.borrow_mut().insert(key, result);
        result
    }

    /// Whether `e` anchors any relative selector of a `:has()` list,
    /// memoized by the list's address (the result never depends on an outer
    /// anchor: `RelativeAnchor` only appears in the list's own selectors).
    fn has(&self, list: &RelativeSelectorList, e: E) -> bool {
        let key = (std::ptr::from_ref(list) as usize, e);
        if let Some(&known) = self.cache.lists.borrow().get(&key) {
            return known;
        }
        let result = list.selectors().iter().any(|r| self.relative(r, e));
        self.cache.lists.borrow_mut().insert(key, result);
        result
    }

    fn relative(&self, relative: &RelativeSelector, anchor: E) -> bool {
        let selector = relative.selector();
        let compounds = selector.compounds();
        let inner = &selector.combinators()[1..];
        // One written compound: no anchor-dependent state, so the
        // descendant case shares subtree results across anchors.
        if compounds.len() == 2 {
            let compound = &compounds[1];
            let mut siblings =
                std::iter::successors(anchor.next_sibling_element(), |s| s.next_sibling_element());
            return match relative.leading_combinator() {
                Combinator::Child => std::iter::successors(anchor.first_child_element(), |c| {
                    c.next_sibling_element()
                })
                .any(|c| compound_matches(compound, c, self)),
                Combinator::Descendant => self.subtree_any(compound, anchor),
                Combinator::NextSibling => anchor
                    .next_sibling_element()
                    .is_some_and(|s| compound_matches(compound, s, self)),
                Combinator::SubsequentSibling => {
                    siblings.any(|s| compound_matches(compound, s, self))
                }
            };
        }
        // Otherwise try every element the subject could be (TD-30).
        let anchored = Matcher {
            cx: self.cx,
            cache: self.cache,
            host: self.host,
            anchor: Some(anchor),
        };
        let descends = inner
            .iter()
            .any(|c| matches!(c, Combinator::Child | Combinator::Descendant));
        let test = |candidate: E| complex(selector, candidate, &anchored);
        match relative.leading_combinator() {
            Combinator::Child if !descends => {
                std::iter::successors(anchor.first_child_element(), |c| c.next_sibling_element())
                    .any(test)
            }
            Combinator::Child | Combinator::Descendant => Descendants::new(anchor).any(test),
            Combinator::NextSibling | Combinator::SubsequentSibling => {
                let mut siblings = std::iter::successors(anchor.next_sibling_element(), |s| {
                    s.next_sibling_element()
                });
                siblings.any(|s| test(s) || (descends && Descendants::new(s).any(test)))
            }
        }
    }

    /// Whether some descendant of `root` matches `compound`, memoized per
    /// (compound, element) so that `:has(x)` over a whole restyle costs time
    /// linear in the tree. The walk is iterative.
    fn subtree_any(&self, compound: &Compound, root: E) -> bool {
        let address = std::ptr::from_ref(compound) as usize;
        if let Some(&known) = self.cache.subtrees.borrow().get(&(address, root)) {
            return known;
        }
        // Frames: (element, next child to visit, found so far).
        let mut stack = vec![(root, root.first_child_element(), false)];
        loop {
            let top = stack.last_mut().expect("nonempty stack");
            if let Some(child) = top.1 {
                top.1 = child.next_sibling_element();
                if top.2 {
                    continue;
                }
                if compound_matches(compound, child, self) {
                    stack.last_mut().expect("nonempty stack").2 = true;
                    continue;
                }
                let known = self.cache.subtrees.borrow().get(&(address, child)).copied();
                match known {
                    Some(found) => stack.last_mut().expect("nonempty stack").2 |= found,
                    None => stack.push((child, child.first_child_element(), false)),
                }
            } else {
                let (element, _, found) = stack.pop().expect("nonempty stack");
                self.cache
                    .subtrees
                    .borrow_mut()
                    .insert((address, element), found);
                match stack.last_mut() {
                    Some(parent) => parent.2 |= found,
                    None => return found,
                }
            }
        }
    }
}

fn complex<E: ElementView>(selector: &ComplexSelector, element: E, m: &Matcher<'_, E>) -> bool {
    if let Some(pseudo) = selector.pseudo_element() {
        if !selector
            .pseudo_element_states()
            .iter()
            .all(|state| pseudo_state_matches(state, pseudo, element))
        {
            return false;
        }
    }
    let compounds = selector.compounds();
    let combinators = selector.combinators();
    let last = compounds.len() - 1;
    // Index `j` counts compounds from the subject (j = 0) leftwards.
    let compound = |j: usize| &compounds[last - j];
    let combinator = |j: usize| combinators[last - j - 1];
    if !compound_matches(compound(0), element, m) {
        return false;
    }
    if last == 0 {
        return true;
    }
    // Each frame: compound `j` matched; `cursor` is the element last tried
    // for compound `j + 1` through combinator `j`.
    let mut stack: Vec<(usize, E)> = vec![(0, element)];
    let mut returned: Option<Outcome> = None;
    loop {
        let (j, cursor) = *stack.last().expect("nonempty stack");
        let comb = combinator(j);
        if let Some(outcome) = returned.take() {
            let give_up = match (outcome, comb) {
                (Outcome::Matched, _) => true,
                (Outcome::Global, _) | (_, Combinator::NextSibling) => true,
                (_, Combinator::Child) => {
                    returned = Some(Outcome::RestartFromDescendant);
                    stack.pop();
                    if stack.is_empty() {
                        return false;
                    }
                    continue;
                }
                (Outcome::RestartFromDescendant, Combinator::SubsequentSibling) => true,
                _ => false,
            };
            if give_up {
                stack.pop();
                if stack.is_empty() {
                    return outcome == Outcome::Matched;
                }
                returned = Some(outcome);
                continue;
            }
        }
        let next = match comb {
            // Nothing lies beyond the host of the tree being matched.
            _ if m.host == Some(cursor) => None,
            Combinator::Child | Combinator::Descendant => cursor
                .parent_element()
                .or_else(|| m.host.and_then(|_| cursor.parent_shadow_host())),
            Combinator::NextSibling | Combinator::SubsequentSibling => {
                cursor.previous_sibling_element()
            }
        };
        let Some(candidate) = next else {
            stack.pop();
            if stack.is_empty() {
                return false;
            }
            returned = Some(match comb {
                Combinator::Child | Combinator::Descendant => Outcome::Global,
                _ => Outcome::RestartFromDescendant,
            });
            continue;
        };
        stack.last_mut().expect("nonempty stack").1 = candidate;
        returned = Some(if !compound_matches(compound(j + 1), candidate, m) {
            Outcome::RestartFromSibling
        } else if j + 1 == last {
            Outcome::Matched
        } else {
            stack.push((j + 1, candidate));
            continue;
        });
    }
}

/// A failed partial match says where matching may usefully resume.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    Matched,
    /// Try the next candidate of the closest sibling combinator to the right.
    RestartFromSibling,
    /// Try the next candidate of the closest descendant combinator.
    RestartFromDescendant,
    /// No placement of the remaining compounds can match.
    Global,
}

fn compound_matches<E: ElementView>(compound: &Compound, e: E, m: &Matcher<'_, E>) -> bool {
    compound
        .simple_selectors()
        .iter()
        .all(|simple| simple_matches(simple, e, m))
}

fn is_html<E: ElementView>(e: &E, m: &Matcher<'_, E>) -> bool {
    m.cx.html_document && e.namespace() == Some(HTML_NAMESPACE)
}

fn namespace_matches(constraint: &NamespaceSelector, namespace: Option<&str>) -> bool {
    match constraint {
        NamespaceSelector::Implicit(None) | NamespaceSelector::Any => true,
        NamespaceSelector::Implicit(Some(url)) | NamespaceSelector::Prefix { url, .. } => {
            namespace == (!url.is_empty()).then_some(url.as_str())
        }
        NamespaceSelector::None => namespace.is_none(),
    }
}

fn attribute_value<'a, E: ElementView>(e: &'a E, local: &str) -> Option<&'a [u16]> {
    e.attributes()
        .find(|a| a.namespace.is_none() && a.local == local)
        .map(|a| a.value)
}

fn simple_matches<E: ElementView>(simple: &SimpleSelector, e: E, m: &Matcher<'_, E>) -> bool {
    if e.is_virtual_scope() {
        return match simple {
            SimpleSelector::PseudoClass(PseudoClass::Scope) | SimpleSelector::Nesting(None) => {
                m.cx.scope == Some(e)
            }
            SimpleSelector::PseudoClass(PseudoClass::Is(list) | PseudoClass::Where(list)) => {
                m.list(list, e)
            }
            _ => false,
        };
    }
    if m.host == Some(e) {
        // The featureless host.
        return match simple {
            SimpleSelector::PseudoClass(
                class @ (PseudoClass::Host(_) | PseudoClass::Is(_) | PseudoClass::Where(_)),
            ) => pseudo_class_matches(class, e, m),
            SimpleSelector::PseudoClass(PseudoClass::Scope) | SimpleSelector::Nesting(None) => {
                m.cx.scope == Some(e)
            }
            _ => false,
        };
    }
    match simple {
        SimpleSelector::Type { namespace, local } => {
            namespace_matches(namespace, e.namespace())
                && if is_html(&e, m) {
                    e.local_name().eq_ignore_ascii_case(local)
                        && !e.local_name().bytes().any(|b| b.is_ascii_uppercase())
                } else {
                    e.local_name() == local
                }
        }
        SimpleSelector::Universal { namespace } => namespace_matches(namespace, e.namespace()),
        SimpleSelector::Id(id) => {
            attribute_value(&e, "id").is_some_and(|v| text::equals(v, id, m.cx.quirks))
        }
        SimpleSelector::Class(class) => {
            attribute_value(&e, "class").is_some_and(|v| text::includes(v, class, m.cx.quirks))
        }
        SimpleSelector::Attribute(attribute) => attribute_matches(attribute, e, m),
        SimpleSelector::PseudoClass(class) => pseudo_class_matches(class, e, m),
        SimpleSelector::Nesting(Some(parent)) => m.list(parent, e),
        SimpleSelector::Nesting(None) => scope_matches(e, m),
        SimpleSelector::RelativeAnchor => m.anchor == Some(e),
    }
}

fn scope_matches<E: ElementView>(e: E, m: &Matcher<'_, E>) -> bool {
    m.cx.scope.map_or_else(|| e.is_root(), |scope| scope == e)
}

/// A user-action pseudo-class after a pseudo-element, tested against the
/// pseudo-element's state (the parser allows nothing else there).
fn pseudo_state_matches<E: ElementView>(class: &PseudoClass, pseudo: PseudoElement, e: E) -> bool {
    let compound_holds = |list: &SelectorList| {
        list.selectors().iter().any(|s| {
            s.compounds().iter().all(|c| {
                c.simple_selectors().iter().all(|simple| match simple {
                    SimpleSelector::PseudoClass(inner) => pseudo_state_matches(inner, pseudo, e),
                    _ => false,
                })
            })
        })
    };
    match class {
        PseudoClass::State(state) => e.pseudo_element_in_state(pseudo, *state),
        PseudoClass::Is(list) | PseudoClass::Where(list) => compound_holds(list),
        PseudoClass::Not(list) => !compound_holds(list),
        _ => false,
    }
}

/// HTML's attributes whose values selectors compare ASCII case-insensitively
/// on HTML elements (HTML Standard §4.16.2, "Case-sensitivity of selectors").
const CASE_INSENSITIVE_HTML_ATTRIBUTES: &[&str] = &[
    "accept",
    "accept-charset",
    "align",
    "alink",
    "axis",
    "bgcolor",
    "charset",
    "checked",
    "clear",
    "codetype",
    "color",
    "compact",
    "declare",
    "defer",
    "dir",
    "direction",
    "disabled",
    "enctype",
    "face",
    "frame",
    "hreflang",
    "http-equiv",
    "lang",
    "language",
    "link",
    "media",
    "method",
    "multiple",
    "nohref",
    "noresize",
    "noshade",
    "nowrap",
    "readonly",
    "rel",
    "rev",
    "rules",
    "scope",
    "scrolling",
    "selected",
    "shape",
    "target",
    "text",
    "type",
    "valign",
    "valuetype",
    "vlink",
];

fn attribute_matches<E: ElementView>(
    attribute: &AttributeSelector,
    e: E,
    m: &Matcher<'_, E>,
) -> bool {
    let html = is_html(&e, m);
    let lowered;
    let name = if html {
        lowered = attribute.local.to_ascii_lowercase();
        lowered.as_str()
    } else {
        attribute.local.as_str()
    };
    let namespace_ok = |ns: Option<&str>| match &attribute.namespace {
        // Unprefixed attribute selectors select unnamespaced attributes.
        NamespaceSelector::Implicit(_) => ns.is_none(),
        other => namespace_matches(other, ns),
    };
    e.attributes()
        .filter(|a| a.local == name && namespace_ok(a.namespace))
        .any(|a| {
            let Some(test) = &attribute.value else {
                return true;
            };
            let insensitive = match test.case {
                AttributeCase::Insensitive => true,
                AttributeCase::Sensitive => false,
                AttributeCase::Default => {
                    html && a.namespace.is_none()
                        && CASE_INSENSITIVE_HTML_ATTRIBUTES.contains(&name)
                }
            };
            let (v, s) = (a.value, test.value.as_str());
            match test.operator {
                AttributeOperator::Equals => text::equals(v, s, insensitive),
                AttributeOperator::Includes => {
                    !s.is_empty()
                        && !s.encode_utf16().any(text::is_html_whitespace)
                        && text::includes(v, s, insensitive)
                }
                AttributeOperator::DashMatch => text::dash_match(v, s, insensitive),
                AttributeOperator::Prefix => !s.is_empty() && text::starts_with(v, s, insensitive),
                AttributeOperator::Suffix => !s.is_empty() && text::ends_with(v, s, insensitive),
                AttributeOperator::Substring => !s.is_empty() && text::contains(v, s, insensitive),
            }
        })
}

fn same_type<E: ElementView>(a: &E, b: &E) -> bool {
    a.local_name() == b.local_name() && a.namespace() == b.namespace()
}

fn is_link<E: ElementView>(e: &E) -> bool {
    e.namespace() == Some(HTML_NAMESPACE)
        && matches!(e.local_name(), "a" | "area")
        && attribute_value(e, "href").is_some()
}

fn nth_matches<E: ElementView>(nth: &Nth, e: E, m: &Matcher<'_, E>) -> bool {
    if let Some(of) = &nth.of {
        if !m.list(of, e) {
            return false;
        }
    }
    let counts = |sibling: &E| match nth.kind {
        NthKind::Child | NthKind::LastChild => {
            nth.of.as_ref().is_none_or(|of| m.list(of, *sibling))
        }
        NthKind::OfType | NthKind::LastOfType => same_type(sibling, &e),
    };
    let backwards = matches!(nth.kind, NthKind::LastChild | NthKind::LastOfType);
    let step = |s: &E| {
        if backwards {
            s.next_sibling_element()
        } else {
            s.previous_sibling_element()
        }
    };
    let key: NthKey = (
        match nth.kind {
            NthKind::Child => 0,
            NthKind::LastChild => 1,
            NthKind::OfType => 2,
            NthKind::LastOfType => 3,
        },
        nth.of
            .as_deref()
            .map_or(0, |of| std::ptr::from_ref(of) as usize),
    );
    // A counted sibling's position is one less than the next counted
    // sibling's, so walk towards the start only until a known position, then
    // record every counted sibling passed on the way.
    let mut passed = vec![e];
    let mut base: u64 = 0;
    let mut sibling = step(&e);
    while let Some(s) = sibling {
        // `counts` may match nested selectors that use this cache too, so no
        // borrow is held across it.
        if counts(&s) {
            if let Some(&known) = m.cache.positions.borrow().get(&(key, s)) {
                base = known;
                break;
            }
            passed.push(s);
        }
        sibling = step(&s);
    }
    let mut positions = m.cache.positions.borrow_mut();
    let mut position = base;
    for s in passed.iter().rev() {
        position = position.saturating_add(1);
        positions.insert((key, *s), position);
    }
    nth.matches_position(position)
}

fn pseudo_class_matches<E: ElementView>(class: &PseudoClass, e: E, m: &Matcher<'_, E>) -> bool {
    match class {
        PseudoClass::Root => e.is_root(),
        PseudoClass::Empty => e.is_empty(),
        PseudoClass::Scope => scope_matches(e, m),
        PseudoClass::OnlyChild => {
            e.previous_sibling_element().is_none() && e.next_sibling_element().is_none()
        }
        PseudoClass::OnlyOfType => {
            let unique = |mut s: Option<E>, next: fn(&E) -> Option<E>| {
                while let Some(sibling) = s {
                    if same_type(&sibling, &e) {
                        return false;
                    }
                    s = next(&sibling);
                }
                true
            };
            unique(e.previous_sibling_element(), E::previous_sibling_element)
                && unique(e.next_sibling_element(), E::next_sibling_element)
        }
        PseudoClass::Nth { nth, .. } => nth_matches(nth, e, m),
        PseudoClass::Is(list) | PseudoClass::Where(list) => m.list(list, e),
        PseudoClass::Not(list) => !m.list(list, e),
        PseudoClass::AnyLink { .. } => is_link(&e),
        PseudoClass::Visited => false,
        PseudoClass::State(state) => e.in_state(*state),
        PseudoClass::Host(argument) => {
            m.host == Some(e)
                && argument.as_deref().is_none_or(|list| {
                    // The argument sees the host with its features.
                    let plain = Matcher {
                        cx: m.cx,
                        cache: m.cache,
                        host: None,
                        anchor: m.anchor,
                    };
                    plain.list(list, e)
                })
        }
        PseudoClass::Has(list) => m.has(list, e),
        PseudoClass::Form(state) => html_state::form_state(*state, e, m.cache),
        PseudoClass::Lang(ranges) => html_state::lang_matches(ranges, e, m.cache),
        PseudoClass::Dir(dir) => match dir.as_str() {
            "ltr" => html_state::directionality(e, m.cache) == StrongDirection::Ltr,
            "rtl" => html_state::directionality(e, m.cache) == StrongDirection::Rtl,
            _ => false,
        },
    }
}
