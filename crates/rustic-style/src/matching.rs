//! Right-to-left selector matching (Selectors Level 4 §18).

use rustic_css::{
    AttributeCase, AttributeOperator, AttributeSelector, Combinator, ComplexSelector, Compound,
    NamespaceSelector, Nth, NthKind, PseudoClass, SelectorList, SimpleSelector,
};
use rustic_dom::HTML_NAMESPACE;

use crate::ElementView;
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
    list.selectors()
        .iter()
        .any(|s| s.pseudo_element().is_none() && matches_complex(s, element, cx))
}

/// Whether `element` is the subject (originating element, for a selector
/// with a pseudo-element) of `selector`.
pub fn matches_complex<E: ElementView>(
    selector: &ComplexSelector,
    element: E,
    cx: &MatchContext<E>,
) -> bool {
    let compounds = selector.compounds();
    let combinators = selector.combinators();
    let last = compounds.len() - 1;
    // Index `j` counts compounds from the subject (j = 0) leftwards.
    let compound = |j: usize| &compounds[last - j];
    let combinator = |j: usize| combinators[last - j - 1];
    if !compound_matches(compound(0), element, cx) {
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
            Combinator::Child | Combinator::Descendant => cursor.parent_element(),
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
        returned = Some(if !compound_matches(compound(j + 1), candidate, cx) {
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

fn compound_matches<E: ElementView>(compound: &Compound, e: E, cx: &MatchContext<E>) -> bool {
    compound
        .simple_selectors()
        .iter()
        .all(|simple| simple_matches(simple, e, cx))
}

fn is_html<E: ElementView>(e: &E, cx: &MatchContext<E>) -> bool {
    cx.html_document && e.namespace() == Some(HTML_NAMESPACE)
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

fn simple_matches<E: ElementView>(simple: &SimpleSelector, e: E, cx: &MatchContext<E>) -> bool {
    match simple {
        SimpleSelector::Type { namespace, local } => {
            namespace_matches(namespace, e.namespace())
                && if is_html(&e, cx) {
                    e.local_name().eq_ignore_ascii_case(local)
                        && !e.local_name().bytes().any(|b| b.is_ascii_uppercase())
                } else {
                    e.local_name() == local
                }
        }
        SimpleSelector::Universal { namespace } => namespace_matches(namespace, e.namespace()),
        SimpleSelector::Id(id) => {
            attribute_value(&e, "id").is_some_and(|v| text::equals(v, id, cx.quirks))
        }
        SimpleSelector::Class(class) => {
            attribute_value(&e, "class").is_some_and(|v| text::includes(v, class, cx.quirks))
        }
        SimpleSelector::Attribute(attribute) => attribute_matches(attribute, e, cx),
        SimpleSelector::PseudoClass(class) => pseudo_class_matches(class, e, cx),
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
    cx: &MatchContext<E>,
) -> bool {
    let html = is_html(&e, cx);
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

fn nth_matches<E: ElementView>(nth: &Nth, e: E, cx: &MatchContext<E>) -> bool {
    if let Some(of) = &nth.of {
        if !matches(of, e, cx) {
            return false;
        }
    }
    let counts = |sibling: &E| match nth.kind {
        NthKind::Child | NthKind::LastChild => {
            nth.of.as_ref().is_none_or(|of| matches(of, *sibling, cx))
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
    let mut position: u64 = 1;
    let mut sibling = step(&e);
    while let Some(s) = sibling {
        if counts(&s) {
            position += 1;
        }
        sibling = step(&s);
    }
    nth.matches_position(position)
}

fn pseudo_class_matches<E: ElementView>(class: &PseudoClass, e: E, cx: &MatchContext<E>) -> bool {
    match class {
        PseudoClass::Root => e.is_root(),
        PseudoClass::Empty => e.is_empty(),
        PseudoClass::Scope => cx.scope.map_or_else(|| e.is_root(), |scope| scope == e),
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
        PseudoClass::Nth { nth, .. } => nth_matches(nth, e, cx),
        PseudoClass::Is(list) | PseudoClass::Where(list) => {
            list.selectors().iter().any(|s| matches_complex(s, e, cx))
        }
        PseudoClass::Not(list) => !list.selectors().iter().any(|s| matches_complex(s, e, cx)),
        PseudoClass::AnyLink { .. } => is_link(&e),
        PseudoClass::Visited => false,
        PseudoClass::State(state) => e.in_state(*state),
    }
}
