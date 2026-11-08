//! Selectors Level 4 syntax and specificity (the CSS-02 subset).
//!
//! Supported: type and universal selectors with namespace prefixes, ID,
//! class and attribute selectors (all six operators and the `i`/`s` flags),
//! the four combinators, the structural pseudo-classes (including
//! `:nth-child(An+B of S)`), `:is()`, `:where()`, `:not()`, `:scope`, the
//! link and user-action pseudo-classes, and the `::before`, `::after`,
//! `::first-line`, `::first-letter` and `::marker` pseudo-elements.
//!
//! Anything else, such as `:has()`, the nesting selector `&` or the column
//! combinator, is rejected with a [`SelectorErrorKind`] instead of being
//! accepted and silently never matching. Matching lives in `rustic-style`.

use std::fmt::{self, Write};

use rustic_base::SourceSpan;

use crate::serialize::{serialize_identifier, serialize_string};
use crate::{BlockKind, ComponentValue, Token, Value, parse_an_plus_b};

/// How deeply `:is()`, `:where()`, `:not()` and `:nth-*(of S)` may nest.
///
/// This bounds the recursion of both parsing and matching.
pub const MAX_SELECTOR_NESTING: usize = 32;

/// `@namespace` declarations in scope for a stylesheet.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NamespaceMap {
    default: Option<String>,
    prefixes: Vec<(String, String)>,
}

impl NamespaceMap {
    /// No default namespace and no prefixes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the default namespace applied to unprefixed type selectors.
    pub fn with_default(mut self, url: impl Into<String>) -> Self {
        self.default = Some(url.into());
        self
    }

    /// Declare a prefix; a later declaration of the same prefix replaces it.
    pub fn with_prefix(mut self, prefix: impl Into<String>, url: impl Into<String>) -> Self {
        let prefix = prefix.into();
        let url = url.into();
        match self.prefixes.iter_mut().find(|(p, _)| *p == prefix) {
            Some(entry) => entry.1 = url,
            None => self.prefixes.push((prefix, url)),
        }
        self
    }

    /// The default namespace, if any.
    pub fn default_namespace(&self) -> Option<&str> {
        self.default.as_deref()
    }

    /// The namespace URL declared for `prefix` (case-sensitive).
    pub fn resolve(&self, prefix: &str) -> Option<&str> {
        self.prefixes
            .iter()
            .find(|(p, _)| p == prefix)
            .map(|(_, url)| url.as_str())
    }
}

/// Why a selector failed to parse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectorErrorKind {
    /// An empty selector or an empty entry in a selector list.
    Empty,
    /// A token that cannot appear at this position.
    UnexpectedToken,
    /// A combinator with nothing after it.
    DanglingCombinator,
    /// A malformed `[attribute]` selector.
    InvalidAttribute,
    /// A namespace prefix with no `@namespace` declaration.
    UnknownNamespacePrefix,
    /// A pseudo-class that does not exist or is not implemented yet.
    UnsupportedPseudoClass,
    /// A pseudo-element that does not exist or is not implemented yet.
    UnsupportedPseudoElement,
    /// Invalid `An+B` arguments to an `:nth-*()` pseudo-class.
    InvalidAnPlusB,
    /// A pseudo-element that is not at the end of a top-level selector.
    MisplacedPseudoElement,
    /// Pseudo-class arguments nested beyond [`MAX_SELECTOR_NESTING`].
    NestingLimit,
    /// The nesting selector `&` or column combinator `||` (not implemented).
    Unsupported,
}

/// A selector parse error and the source range it was found at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectorError {
    /// What went wrong.
    pub kind: SelectorErrorKind,
    /// Byte range of the offending component value (empty at end of input).
    pub span: SourceSpan,
}

impl fmt::Display for SelectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let range = self.span.range();
        write!(
            f,
            "invalid selector ({:?}) at bytes {}..{}",
            self.kind, range.start, range.end
        )
    }
}

impl std::error::Error for SelectorError {}

/// Selector specificity `(A, B, C)`, compared lexicographically.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Specificity {
    /// ID selectors.
    pub ids: u32,
    /// Class, attribute and pseudo-class selectors.
    pub classes: u32,
    /// Type selectors and pseudo-elements.
    pub types: u32,
}

impl Specificity {
    fn add(self, other: Self) -> Self {
        Self {
            ids: self.ids.saturating_add(other.ids),
            classes: self.classes.saturating_add(other.classes),
            types: self.types.saturating_add(other.types),
        }
    }

    const ID: Self = Self {
        ids: 1,
        classes: 0,
        types: 0,
    };
    const CLASS: Self = Self {
        ids: 0,
        classes: 1,
        types: 0,
    };
    const TYPE: Self = Self {
        ids: 0,
        classes: 0,
        types: 1,
    };
}

/// A namespace constraint on a type, universal or attribute selector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NamespaceSelector {
    /// No prefix was written. For type and universal selectors this means
    /// the default namespace (if one is declared, carried here) or any
    /// namespace; for attribute selectors it means no namespace.
    Implicit(Option<String>),
    /// `*|`: any namespace, including none.
    Any,
    /// `|`: no namespace.
    None,
    /// `prefix|`, resolved through the [`NamespaceMap`].
    Prefix {
        /// The prefix as written, kept for serialization.
        prefix: String,
        /// The namespace URL it resolved to.
        url: String,
    },
}

/// Attribute selector operators.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeOperator {
    /// `=`: exactly equal.
    Equals,
    /// `~=`: one of a whitespace-separated list.
    Includes,
    /// `|=`: equal, or starting with the value followed by `-`.
    DashMatch,
    /// `^=`: starts with a nonempty value.
    Prefix,
    /// `$=`: ends with a nonempty value.
    Suffix,
    /// `*=`: contains a nonempty value.
    Substring,
}

impl AttributeOperator {
    fn as_str(self) -> &'static str {
        match self {
            Self::Equals => "=",
            Self::Includes => "~=",
            Self::DashMatch => "|=",
            Self::Prefix => "^=",
            Self::Suffix => "$=",
            Self::Substring => "*=",
        }
    }
}

/// Attribute value case sensitivity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeCase {
    /// No flag: the document language decides (HTML has a legacy list of
    /// ASCII case-insensitive attributes).
    Default,
    /// The `i` flag: ASCII case-insensitive.
    Insensitive,
    /// The `s` flag: case-sensitive even for HTML's legacy attributes.
    Sensitive,
}

/// A value test inside an attribute selector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeValue {
    /// The operator.
    pub operator: AttributeOperator,
    /// The (unescaped) value to compare against.
    pub value: String,
    /// The case-sensitivity flag.
    pub case: AttributeCase,
}

/// `[namespace|name operator value flag]`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeSelector {
    /// Namespace constraint; [`NamespaceSelector::Implicit`] means none.
    pub namespace: NamespaceSelector,
    /// Attribute local name as written.
    pub local: String,
    /// The value test; `None` tests for presence only.
    pub value: Option<AttributeValue>,
}

/// Which `:nth-*()` pseudo-class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NthKind {
    /// `:nth-child()`, `:first-child`.
    Child,
    /// `:nth-last-child()`, `:last-child`.
    LastChild,
    /// `:nth-of-type()`, `:first-of-type`.
    OfType,
    /// `:nth-last-of-type()`, `:last-of-type`.
    LastOfType,
}

impl NthKind {
    fn name(self) -> &'static str {
        match self {
            Self::Child => "nth-child",
            Self::LastChild => "nth-last-child",
            Self::OfType => "nth-of-type",
            Self::LastOfType => "nth-last-of-type",
        }
    }
}

/// An `:nth-*()` test: the element's 1-based position must equal `A×n+B`
/// for some integer `n ≥ 0`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Nth {
    /// Which siblings are counted, and from which end.
    pub kind: NthKind,
    /// `A`.
    pub a: i32,
    /// `B`.
    pub b: i32,
    /// `of S`: only siblings matching this list count (child kinds only).
    pub of: Option<Box<SelectorList>>,
}

impl Nth {
    /// Whether 1-based `position` is `A×n+B` for some `n ≥ 0`.
    pub fn matches_position(&self, position: u64) -> bool {
        let (a, b) = (i64::from(self.a), i64::from(self.b));
        let Ok(position) = i64::try_from(position) else {
            return false;
        };
        let diff = position - b;
        if a == 0 {
            diff == 0
        } else {
            diff % a == 0 && diff / a >= 0
        }
    }
}

/// User-interaction and document-state pseudo-classes.
///
/// Whether an element is in one of these states is supplied by the caller's
/// element view; with no interaction model yet, every state is off.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElementState {
    /// `:hover`.
    Hover,
    /// `:active`.
    Active,
    /// `:focus`.
    Focus,
    /// `:focus-visible`.
    FocusVisible,
    /// `:focus-within`.
    FocusWithin,
    /// `:target`.
    Target,
}

impl ElementState {
    fn name(self) -> &'static str {
        match self {
            Self::Hover => "hover",
            Self::Active => "active",
            Self::Focus => "focus",
            Self::FocusVisible => "focus-visible",
            Self::FocusWithin => "focus-within",
            Self::Target => "target",
        }
    }
}

/// A supported pseudo-class.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PseudoClass {
    /// `:root`.
    Root,
    /// `:empty`: no element children and no nonempty text.
    Empty,
    /// `:scope`: the scoping root, or `:root` without one.
    Scope,
    /// `:only-child`.
    OnlyChild,
    /// `:only-of-type`.
    OnlyOfType,
    /// `:first-child` and friends are `Nth` with `A = 0, B = 1`; `written`
    /// keeps the keyword form for serialization.
    Nth {
        /// The test.
        nth: Nth,
        /// `true` when written as `:first-child`, `:last-of-type`, etc.
        keyword: bool,
    },
    /// `:is(...)`, a forgiving list.
    Is(SelectorList),
    /// `:where(...)`, a forgiving list with zero specificity.
    Where(SelectorList),
    /// `:not(...)`.
    Not(SelectorList),
    /// `:any-link` and `:link`: `a` and `area` elements with `href`. Visited
    /// state is never exposed, so `:link` matches every link.
    AnyLink {
        /// `true` for `:link`, kept for serialization.
        link: bool,
    },
    /// `:visited`, which never matches (history is not exposed to style).
    Visited,
    /// A user-interaction state.
    State(ElementState),
}

/// A supported pseudo-element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PseudoElement {
    /// `::before`.
    Before,
    /// `::after`.
    After,
    /// `::first-line`.
    FirstLine,
    /// `::first-letter`.
    FirstLetter,
    /// `::marker`.
    Marker,
}

impl PseudoElement {
    fn name(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
            Self::FirstLine => "first-line",
            Self::FirstLetter => "first-letter",
            Self::Marker => "marker",
        }
    }

    fn from_name(name: &str, legacy_only: bool) -> Option<Self> {
        let pe = match name {
            "before" => Self::Before,
            "after" => Self::After,
            "first-line" => Self::FirstLine,
            "first-letter" => Self::FirstLetter,
            "marker" if !legacy_only => Self::Marker,
            _ => return None,
        };
        Some(pe)
    }
}

/// A simple selector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SimpleSelector {
    /// `ns|name`.
    Type {
        /// Namespace constraint.
        namespace: NamespaceSelector,
        /// Local name as written.
        local: String,
    },
    /// `ns|*`.
    Universal {
        /// Namespace constraint.
        namespace: NamespaceSelector,
    },
    /// `#id`.
    Id(String),
    /// `.class`.
    Class(String),
    /// `[attribute]`.
    Attribute(AttributeSelector),
    /// `:pseudo-class`.
    PseudoClass(PseudoClass),
}

/// A sequence of simple selectors with no combinator between them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Compound {
    simple: Vec<SimpleSelector>,
}

impl Compound {
    /// Simple selectors in source order; a type or universal selector, if
    /// present, is first.
    pub fn simple_selectors(&self) -> &[SimpleSelector] {
        &self.simple
    }
}

/// How two compound selectors are related.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Combinator {
    /// Whitespace: an ancestor.
    Descendant,
    /// `>`: the parent.
    Child,
    /// `+`: the immediately preceding sibling.
    NextSibling,
    /// `~`: any preceding sibling.
    SubsequentSibling,
}

/// Compound selectors joined by combinators, with an optional trailing
/// pseudo-element.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComplexSelector {
    compounds: Vec<Compound>,
    combinators: Vec<Combinator>,
    pseudo_element: Option<PseudoElement>,
    specificity: Specificity,
}

impl ComplexSelector {
    /// Compounds in source order (the last one is the subject).
    pub fn compounds(&self) -> &[Compound] {
        &self.compounds
    }

    /// `combinators()[i]` joins `compounds()[i]` and `compounds()[i + 1]`.
    pub fn combinators(&self) -> &[Combinator] {
        &self.combinators
    }

    /// The pseudo-element this selector targets, if any.
    pub fn pseudo_element(&self) -> Option<PseudoElement> {
        self.pseudo_element
    }

    /// Specificity per Selectors Level 4 §17.
    pub fn specificity(&self) -> Specificity {
        self.specificity
    }
}

/// A comma-separated list of complex selectors.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SelectorList {
    selectors: Vec<ComplexSelector>,
}

impl SelectorList {
    /// The selectors in source order. Empty only for a forgiving list such as
    /// `:is()` whose entries were all invalid.
    pub fn selectors(&self) -> &[ComplexSelector] {
        &self.selectors
    }

    /// The highest specificity in the list (zero when empty), as used by
    /// `:is()`, `:not()` and `:nth-child(of S)`.
    pub fn max_specificity(&self) -> Specificity {
        self.selectors
            .iter()
            .map(ComplexSelector::specificity)
            .max()
            .unwrap_or_default()
    }
}

/// Parse a top-level selector list, such as a style rule's prelude.
///
/// The list is not forgiving: one invalid selector invalidates the list, as
/// it does a style rule.
///
/// ```
/// use rustic_css::{CssParser, NamespaceMap, parse_selector_list};
/// let prelude = CssParser::default().component_values("ul > li.item:first-child").unwrap().value;
/// let list = parse_selector_list(&prelude, &NamespaceMap::new()).unwrap();
/// let specificity = list.selectors()[0].specificity();
/// assert_eq!((specificity.ids, specificity.classes, specificity.types), (0, 2, 2));
/// assert_eq!(list.to_string(), "ul > li.item:first-child");
/// ```
pub fn parse_selector_list(
    values: &[Value],
    namespaces: &NamespaceMap,
) -> Result<SelectorList, SelectorError> {
    Parser { namespaces }.list(values, 0, true, false)
}

struct Parser<'a> {
    namespaces: &'a NamespaceMap,
}

fn span_at(values: &[Value], index: usize) -> SourceSpan {
    match values.get(index) {
        Some(value) => value.span,
        None => {
            let end = values.last().map_or(0, |v| v.span.range().end);
            SourceSpan::new(end, end).expect("empty span")
        }
    }
}

fn err<T>(kind: SelectorErrorKind, values: &[Value], index: usize) -> Result<T, SelectorError> {
    Err(SelectorError {
        kind,
        span: span_at(values, index),
    })
}

fn token_at(values: &[Value], index: usize) -> Option<&Token> {
    values.get(index).and_then(Value::token)
}

fn is_delim(values: &[Value], index: usize, c: char) -> bool {
    token_at(values, index) == Some(&Token::Delim(c))
}

fn skip_whitespace(values: &[Value], mut index: usize) -> usize {
    while values.get(index).is_some_and(Value::is_whitespace) {
        index += 1;
    }
    index
}

/// Split at top-level commas, keeping every entry (possibly empty).
fn split_commas(values: &[Value]) -> Vec<&[Value]> {
    values.split(|v| v.token() == Some(&Token::Comma)).collect()
}

impl Parser<'_> {
    fn list(
        &self,
        values: &[Value],
        depth: usize,
        allow_pseudo_element: bool,
        forgiving: bool,
    ) -> Result<SelectorList, SelectorError> {
        if depth > MAX_SELECTOR_NESTING {
            return err(SelectorErrorKind::NestingLimit, values, 0);
        }
        let mut selectors = Vec::new();
        for entry in split_commas(values) {
            match self.complex(entry, depth, allow_pseudo_element) {
                Ok(selector) => selectors.push(selector),
                // A nesting overflow is a resource limit, never forgiven.
                Err(e) if forgiving && e.kind != SelectorErrorKind::NestingLimit => {}
                Err(e) => return Err(e),
            }
        }
        if selectors.is_empty() && !forgiving {
            return err(SelectorErrorKind::Empty, values, 0);
        }
        Ok(SelectorList { selectors })
    }

    fn complex(
        &self,
        values: &[Value],
        depth: usize,
        allow_pseudo_element: bool,
    ) -> Result<ComplexSelector, SelectorError> {
        let mut i = skip_whitespace(values, 0);
        if i == values.len() {
            return err(SelectorErrorKind::Empty, values, i);
        }
        let mut compounds = Vec::new();
        let mut combinators = Vec::new();
        let mut specificity = Specificity::default();
        let pseudo_element = loop {
            let (compound, compound_specificity, pseudo_element, next) =
                self.compound(values, i, depth, allow_pseudo_element)?;
            compounds.push(compound);
            specificity = specificity.add(compound_specificity);
            let after_space = skip_whitespace(values, next);
            if pseudo_element.is_some() {
                if after_space != values.len() {
                    return err(SelectorErrorKind::MisplacedPseudoElement, values, next);
                }
                specificity = specificity.add(Specificity::TYPE);
                break pseudo_element;
            }
            if after_space == values.len() {
                break None;
            }
            let combinator = match token_at(values, after_space) {
                Some(Token::Delim('>')) => Some(Combinator::Child),
                Some(Token::Delim('+')) => Some(Combinator::NextSibling),
                Some(Token::Delim('~')) => Some(Combinator::SubsequentSibling),
                Some(Token::Delim('|')) if is_delim(values, after_space + 1, '|') => {
                    return err(SelectorErrorKind::Unsupported, values, after_space);
                }
                _ => None,
            };
            i = match combinator {
                Some(combinator) => {
                    combinators.push(combinator);
                    let next = skip_whitespace(values, after_space + 1);
                    if next == values.len() {
                        return err(SelectorErrorKind::DanglingCombinator, values, after_space);
                    }
                    next
                }
                None if after_space > next => {
                    combinators.push(Combinator::Descendant);
                    after_space
                }
                None => return err(SelectorErrorKind::UnexpectedToken, values, next),
            };
        };
        Ok(ComplexSelector {
            compounds,
            combinators,
            pseudo_element,
            specificity,
        })
    }

    /// Parse `[namespace|]name` or `[namespace|]*` starting at `i`.
    ///
    /// Returns the namespace, `Some(local)` or `None` for `*`, and the next
    /// index; `Ok(None)` when no type selector starts here.
    fn type_selector(
        &self,
        values: &[Value],
        i: usize,
    ) -> Result<Option<(NamespaceSelector, Option<String>, usize)>, SelectorError> {
        let name_at = |j: usize| match token_at(values, j) {
            Some(Token::Ident(name)) => Some(Some(name.clone())),
            Some(Token::Delim('*')) => Some(None),
            _ => None,
        };
        let first = name_at(i);
        if let Some(prefix) = first
            .clone()
            .filter(|_| is_delim(values, i + 1, '|') && !is_delim(values, i + 2, '='))
        {
            let Some(local) = name_at(i + 2) else {
                return err(SelectorErrorKind::UnexpectedToken, values, i + 2);
            };
            let namespace = match prefix {
                None => NamespaceSelector::Any,
                Some(prefix) => match self.namespaces.resolve(&prefix) {
                    Some(url) => NamespaceSelector::Prefix {
                        prefix,
                        url: url.to_owned(),
                    },
                    None => return err(SelectorErrorKind::UnknownNamespacePrefix, values, i),
                },
            };
            return Ok(Some((namespace, local, i + 3)));
        }
        if is_delim(values, i, '|') {
            let Some(local) = name_at(i + 1) else {
                return err(SelectorErrorKind::UnexpectedToken, values, i + 1);
            };
            return Ok(Some((NamespaceSelector::None, local, i + 2)));
        }
        let implicit =
            NamespaceSelector::Implicit(self.namespaces.default_namespace().map(str::to_owned));
        Ok(first.map(|local| (implicit, local, i + 1)))
    }

    fn compound(
        &self,
        values: &[Value],
        start: usize,
        depth: usize,
        allow_pseudo_element: bool,
    ) -> Result<(Compound, Specificity, Option<PseudoElement>, usize), SelectorError> {
        let mut simple = Vec::new();
        let mut specificity = Specificity::default();
        let mut i = start;
        if let Some((namespace, local, next)) = self.type_selector(values, i)? {
            simple.push(match local {
                Some(local) => {
                    specificity = specificity.add(Specificity::TYPE);
                    SimpleSelector::Type { namespace, local }
                }
                None => SimpleSelector::Universal { namespace },
            });
            i = next;
        }
        let mut pseudo_element = None;
        while let Some(value) = values.get(i) {
            let item = match &value.value {
                ComponentValue::Token(Token::Hash { value, id: true }) => {
                    i += 1;
                    specificity = specificity.add(Specificity::ID);
                    SimpleSelector::Id(value.clone())
                }
                ComponentValue::Token(Token::Delim('.')) => {
                    let Some(Token::Ident(name)) = token_at(values, i + 1) else {
                        return err(SelectorErrorKind::UnexpectedToken, values, i);
                    };
                    i += 2;
                    specificity = specificity.add(Specificity::CLASS);
                    SimpleSelector::Class(name.clone())
                }
                ComponentValue::Block {
                    kind: BlockKind::Square,
                    contents,
                } => {
                    let attribute = self.attribute(contents).map_err(|kind| SelectorError {
                        kind,
                        span: value.span,
                    })?;
                    i += 1;
                    specificity = specificity.add(Specificity::CLASS);
                    SimpleSelector::Attribute(attribute)
                }
                ComponentValue::Token(Token::Colon) => {
                    let double = token_at(values, i + 1) == Some(&Token::Colon);
                    let name_index = if double { i + 2 } else { i + 1 };
                    let legacy = match token_at(values, name_index) {
                        Some(Token::Ident(name)) => {
                            PseudoElement::from_name(&name.to_ascii_lowercase(), !double)
                        }
                        _ => None,
                    };
                    if double || legacy.is_some() {
                        let Some(pe) = legacy else {
                            return err(
                                SelectorErrorKind::UnsupportedPseudoElement,
                                values,
                                name_index,
                            );
                        };
                        if !allow_pseudo_element {
                            return err(SelectorErrorKind::MisplacedPseudoElement, values, i);
                        }
                        pseudo_element = Some(pe);
                        i = name_index + 1;
                        break;
                    }
                    let (class, class_specificity) =
                        self.pseudo_class(values, name_index, depth)?;
                    i = name_index + 1;
                    specificity = specificity.add(class_specificity);
                    SimpleSelector::PseudoClass(class)
                }
                ComponentValue::Token(Token::Hash { id: false, .. }) => {
                    return err(SelectorErrorKind::UnexpectedToken, values, i);
                }
                ComponentValue::Token(Token::Delim('&')) => {
                    return err(SelectorErrorKind::Unsupported, values, i);
                }
                _ => break,
            };
            simple.push(item);
        }
        if simple.is_empty() && pseudo_element.is_none() {
            return err(SelectorErrorKind::UnexpectedToken, values, start);
        }
        Ok((Compound { simple }, specificity, pseudo_element, i))
    }

    fn attribute(&self, contents: &[Value]) -> Result<AttributeSelector, SelectorErrorKind> {
        let invalid = SelectorErrorKind::InvalidAttribute;
        let mut i = skip_whitespace(contents, 0);
        let ident = |j: usize| match token_at(contents, j) {
            Some(Token::Ident(name)) => Some(name.clone()),
            _ => None,
        };
        let (namespace, local) = if is_delim(contents, i, '|') {
            i += 2;
            (NamespaceSelector::None, ident(i - 1).ok_or(invalid)?)
        } else if is_delim(contents, i, '*') {
            if !is_delim(contents, i + 1, '|') {
                return Err(invalid);
            }
            i += 3;
            (NamespaceSelector::Any, ident(i - 1).ok_or(invalid)?)
        } else {
            let name = ident(i).ok_or(invalid)?;
            if is_delim(contents, i + 1, '|') && ident(i + 2).is_some() {
                let url = self
                    .namespaces
                    .resolve(&name)
                    .ok_or(SelectorErrorKind::UnknownNamespacePrefix)?;
                let namespace = NamespaceSelector::Prefix {
                    prefix: name,
                    url: url.to_owned(),
                };
                i += 3;
                (namespace, ident(i - 1).expect("checked"))
            } else {
                i += 1;
                (NamespaceSelector::Implicit(None), name)
            }
        };
        i = skip_whitespace(contents, i);
        if i == contents.len() {
            return Ok(AttributeSelector {
                namespace,
                local,
                value: None,
            });
        }
        let operator = match token_at(contents, i) {
            Some(Token::Delim('=')) => {
                i += 1;
                AttributeOperator::Equals
            }
            Some(Token::Delim(c)) if is_delim(contents, i + 1, '=') => {
                let operator = match c {
                    '~' => AttributeOperator::Includes,
                    '|' => AttributeOperator::DashMatch,
                    '^' => AttributeOperator::Prefix,
                    '$' => AttributeOperator::Suffix,
                    '*' => AttributeOperator::Substring,
                    _ => return Err(invalid),
                };
                i += 2;
                operator
            }
            _ => return Err(invalid),
        };
        i = skip_whitespace(contents, i);
        let value = match token_at(contents, i) {
            Some(Token::Ident(v) | Token::String(v)) => v.clone(),
            _ => return Err(invalid),
        };
        i = skip_whitespace(contents, i + 1);
        let case = match token_at(contents, i) {
            None => AttributeCase::Default,
            Some(Token::Ident(flag)) if flag.eq_ignore_ascii_case("i") => {
                AttributeCase::Insensitive
            }
            Some(Token::Ident(flag)) if flag.eq_ignore_ascii_case("s") => AttributeCase::Sensitive,
            Some(_) => return Err(invalid),
        };
        if case != AttributeCase::Default && skip_whitespace(contents, i + 1) != contents.len() {
            return Err(invalid);
        }
        Ok(AttributeSelector {
            namespace,
            local,
            value: Some(AttributeValue {
                operator,
                value,
                case,
            }),
        })
    }

    /// Parse the pseudo-class whose name (ident or function) is at `i`.
    fn pseudo_class(
        &self,
        values: &[Value],
        i: usize,
        depth: usize,
    ) -> Result<(PseudoClass, Specificity), SelectorError> {
        let unsupported = || err(SelectorErrorKind::UnsupportedPseudoClass, values, i);
        let keyword = |kind| PseudoClass::Nth {
            nth: Nth {
                kind,
                a: 0,
                b: 1,
                of: None,
            },
            keyword: true,
        };
        let Some(value) = values.get(i) else {
            return err(SelectorErrorKind::UnexpectedToken, values, i);
        };
        let class = match &value.value {
            ComponentValue::Token(Token::Ident(name)) => match name.to_ascii_lowercase().as_str() {
                "root" => PseudoClass::Root,
                "empty" => PseudoClass::Empty,
                "scope" => PseudoClass::Scope,
                "first-child" => keyword(NthKind::Child),
                "last-child" => keyword(NthKind::LastChild),
                "only-child" => PseudoClass::OnlyChild,
                "first-of-type" => keyword(NthKind::OfType),
                "last-of-type" => keyword(NthKind::LastOfType),
                "only-of-type" => PseudoClass::OnlyOfType,
                "any-link" => PseudoClass::AnyLink { link: false },
                "link" => PseudoClass::AnyLink { link: true },
                "visited" => PseudoClass::Visited,
                "hover" => PseudoClass::State(ElementState::Hover),
                "active" => PseudoClass::State(ElementState::Active),
                "focus" => PseudoClass::State(ElementState::Focus),
                "focus-visible" => PseudoClass::State(ElementState::FocusVisible),
                "focus-within" => PseudoClass::State(ElementState::FocusWithin),
                "target" => PseudoClass::State(ElementState::Target),
                _ => return unsupported(),
            },
            ComponentValue::Function { name, arguments } => {
                let span_err = |e: SelectorError| {
                    if e.kind == SelectorErrorKind::Empty {
                        SelectorError {
                            kind: e.kind,
                            span: value.span,
                        }
                    } else {
                        e
                    }
                };
                match name.to_ascii_lowercase().as_str() {
                    "is" => {
                        let list = self.list(arguments, depth + 1, false, true)?;
                        let s = list.max_specificity();
                        return Ok((PseudoClass::Is(list), s));
                    }
                    "where" => {
                        let list = self.list(arguments, depth + 1, false, true)?;
                        return Ok((PseudoClass::Where(list), Specificity::default()));
                    }
                    "not" => {
                        let list = self
                            .list(arguments, depth + 1, false, false)
                            .map_err(span_err)?;
                        let s = list.max_specificity();
                        return Ok((PseudoClass::Not(list), s));
                    }
                    "nth-child" => self.nth(NthKind::Child, arguments, value.span, depth)?,
                    "nth-last-child" => {
                        self.nth(NthKind::LastChild, arguments, value.span, depth)?
                    }
                    "nth-of-type" => self.nth(NthKind::OfType, arguments, value.span, depth)?,
                    "nth-last-of-type" => {
                        self.nth(NthKind::LastOfType, arguments, value.span, depth)?
                    }
                    _ => return unsupported(),
                }
            }
            _ => return err(SelectorErrorKind::UnexpectedToken, values, i),
        };
        let specificity = match &class {
            PseudoClass::Nth {
                nth: Nth { of: Some(of), .. },
                ..
            } => Specificity::CLASS.add(of.max_specificity()),
            _ => Specificity::CLASS,
        };
        Ok((class, specificity))
    }

    fn nth(
        &self,
        kind: NthKind,
        arguments: &[Value],
        span: SourceSpan,
        depth: usize,
    ) -> Result<PseudoClass, SelectorError> {
        let invalid = SelectorError {
            kind: SelectorErrorKind::InvalidAnPlusB,
            span,
        };
        let of_index = match kind {
            NthKind::Child | NthKind::LastChild => arguments.iter().enumerate().position(|(j, v)| {
                j > 0
                    && arguments[j - 1].is_whitespace()
                    && matches!(v.token(), Some(Token::Ident(w)) if w.eq_ignore_ascii_case("of"))
            }),
            NthKind::OfType | NthKind::LastOfType => None,
        };
        let (anb, of) = match of_index {
            Some(j) => {
                let rest = &arguments[j + 1..];
                if !rest.first().is_some_and(Value::is_whitespace) {
                    return Err(invalid);
                }
                let list = self.list(rest, depth + 1, false, false).map_err(|e| {
                    if e.kind == SelectorErrorKind::Empty {
                        invalid
                    } else {
                        e
                    }
                })?;
                (&arguments[..j], Some(Box::new(list)))
            }
            None => (arguments, None),
        };
        let (a, b) = parse_an_plus_b(anb).ok_or(invalid)?;
        Ok(PseudoClass::Nth {
            nth: Nth { kind, a, b, of },
            keyword: false,
        })
    }
}

fn write_namespace(f: &mut impl Write, namespace: &NamespaceSelector) -> fmt::Result {
    match namespace {
        NamespaceSelector::Implicit(_) => Ok(()),
        NamespaceSelector::Any => f.write_str("*|"),
        NamespaceSelector::None => f.write_char('|'),
        NamespaceSelector::Prefix { prefix, .. } => {
            serialize_identifier(f, prefix)?;
            f.write_char('|')
        }
    }
}

fn write_an_plus_b(f: &mut impl Write, a: i32, b: i32) -> fmt::Result {
    match a {
        0 => return write!(f, "{b}"),
        1 => f.write_char('n')?,
        -1 => f.write_str("-n")?,
        a => write!(f, "{a}n")?,
    }
    match b {
        0 => Ok(()),
        b if b > 0 => write!(f, "+{b}"),
        b => write!(f, "{b}"),
    }
}

impl fmt::Display for SimpleSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Type { namespace, local } => {
                write_namespace(f, namespace)?;
                serialize_identifier(f, local)
            }
            Self::Universal { namespace } => {
                write_namespace(f, namespace)?;
                f.write_char('*')
            }
            Self::Id(id) => {
                f.write_char('#')?;
                serialize_identifier(f, id)
            }
            Self::Class(class) => {
                f.write_char('.')?;
                serialize_identifier(f, class)
            }
            Self::Attribute(attribute) => {
                f.write_char('[')?;
                write_namespace(f, &attribute.namespace)?;
                serialize_identifier(f, &attribute.local)?;
                if let Some(value) = &attribute.value {
                    f.write_str(value.operator.as_str())?;
                    serialize_string(f, &value.value)?;
                    match value.case {
                        AttributeCase::Default => {}
                        AttributeCase::Insensitive => f.write_str(" i")?,
                        AttributeCase::Sensitive => f.write_str(" s")?,
                    }
                }
                f.write_char(']')
            }
            Self::PseudoClass(class) => write!(f, "{class}"),
        }
    }
}

impl fmt::Display for PseudoClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char(':')?;
        match self {
            Self::Root => f.write_str("root"),
            Self::Empty => f.write_str("empty"),
            Self::Scope => f.write_str("scope"),
            Self::OnlyChild => f.write_str("only-child"),
            Self::OnlyOfType => f.write_str("only-of-type"),
            Self::Nth { nth, keyword: true } => f.write_str(match nth.kind {
                NthKind::Child => "first-child",
                NthKind::LastChild => "last-child",
                NthKind::OfType => "first-of-type",
                NthKind::LastOfType => "last-of-type",
            }),
            Self::Nth {
                nth,
                keyword: false,
            } => {
                write!(f, "{}(", nth.kind.name())?;
                write_an_plus_b(f, nth.a, nth.b)?;
                if let Some(of) = &nth.of {
                    write!(f, " of {of}")?;
                }
                f.write_char(')')
            }
            Self::Is(list) => write!(f, "is({list})"),
            Self::Where(list) => write!(f, "where({list})"),
            Self::Not(list) => write!(f, "not({list})"),
            Self::AnyLink { link: false } => f.write_str("any-link"),
            Self::AnyLink { link: true } => f.write_str("link"),
            Self::Visited => f.write_str("visited"),
            Self::State(state) => f.write_str(state.name()),
        }
    }
}

impl fmt::Display for Compound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, simple) in self.simple.iter().enumerate() {
            // An unprefixed `*` adds nothing when other selectors follow.
            let implicit_universal = matches!(
                simple,
                SimpleSelector::Universal {
                    namespace: NamespaceSelector::Implicit(_)
                }
            );
            if index == 0 && implicit_universal && self.simple.len() > 1 {
                continue;
            }
            write!(f, "{simple}")?;
        }
        Ok(())
    }
}

impl fmt::Display for ComplexSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, compound) in self.compounds.iter().enumerate() {
            if index > 0 {
                f.write_str(match self.combinators[index - 1] {
                    Combinator::Descendant => " ",
                    Combinator::Child => " > ",
                    Combinator::NextSibling => " + ",
                    Combinator::SubsequentSibling => " ~ ",
                })?;
            }
            write!(f, "{compound}")?;
        }
        if let Some(pe) = self.pseudo_element {
            write!(f, "::{}", pe.name())?;
        }
        Ok(())
    }
}

impl fmt::Display for SelectorList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, selector) in self.selectors.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{selector}")?;
        }
        Ok(())
    }
}
