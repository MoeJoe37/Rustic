//! Selectors Level 4 syntax and specificity (the CSS-02 and CSS-07a subsets).
//!
//! Supported: type and universal selectors with namespace prefixes, ID,
//! class and attribute selectors (all six operators and the `i`/`s` flags),
//! the four combinators, the structural pseudo-classes (including
//! `:nth-child(An+B of S)`), `:is()`, `:where()`, `:not()`, `:has()`,
//! `:scope`, the link, user-action, form-state, `:lang()` and `:dir()`
//! pseudo-classes, the CSS Nesting selector `&`, and the tree-abiding and
//! highlight pseudo-elements listed on [`PseudoElement`], optionally followed
//! by user-action pseudo-classes.
//!
//! Anything else, such as the column combinator `||`, is rejected with a
//! [`SelectorErrorKind`] instead of being accepted and silently never
//! matching. Matching lives in `rustic-style`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::{self, Write};
use std::sync::Arc;

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
    /// Pseudo-class arguments (or `&` parent selectors) nested beyond
    /// [`MAX_SELECTOR_NESTING`].
    NestingLimit,
    /// The column combinator `||` (not implemented).
    Unsupported,
    /// A `:host()` argument that is not one compound selector.
    InvalidHostArgument,
    /// A pseudo-class after a pseudo-element that does not allow it (only
    /// user-action pseudo-classes may follow a tree-abiding pseudo-element,
    /// and nothing may follow a highlight pseudo-element).
    InvalidPseudoElementState,
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
    /// Whether this is a user-action pseudo-class, which may follow a
    /// tree-abiding pseudo-element (Selectors Level 4 §3.6.3).
    pub fn is_user_action(self) -> bool {
        !matches!(self, Self::Target)
    }

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

/// HTML form-state pseudo-classes (HTML Standard §4.16.3, "Pseudo-classes").
///
/// Matching derives each state from content attributes and the tree, since
/// there is no user interaction or script yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormState {
    /// `:enabled`.
    Enabled,
    /// `:disabled`.
    Disabled,
    /// `:checked`.
    Checked,
    /// `:indeterminate`.
    Indeterminate,
    /// `:default`.
    Default,
    /// `:required`.
    Required,
    /// `:optional`.
    Optional,
    /// `:read-only`.
    ReadOnly,
    /// `:read-write`.
    ReadWrite,
    /// `:placeholder-shown`.
    PlaceholderShown,
    /// `:open`: an open `details` or `dialog` (HTML §4.16.3).
    Open,
    /// `:modal`: a dialog opened with `showModal()`.
    Modal,
    /// `:popover-open`: a popover in the showing state.
    PopoverOpen,
}

impl FormState {
    fn name(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Checked => "checked",
            Self::Indeterminate => "indeterminate",
            Self::Default => "default",
            Self::Required => "required",
            Self::Optional => "optional",
            Self::ReadOnly => "read-only",
            Self::ReadWrite => "read-write",
            Self::PlaceholderShown => "placeholder-shown",
            Self::Open => "open",
            Self::Modal => "modal",
            Self::PopoverOpen => "popover-open",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "enabled" => Self::Enabled,
            "disabled" => Self::Disabled,
            "checked" => Self::Checked,
            "indeterminate" => Self::Indeterminate,
            "default" => Self::Default,
            "required" => Self::Required,
            "optional" => Self::Optional,
            "read-only" => Self::ReadOnly,
            "read-write" => Self::ReadWrite,
            "placeholder-shown" => Self::PlaceholderShown,
            "open" => Self::Open,
            "modal" => Self::Modal,
            "popover-open" => Self::PopoverOpen,
            _ => return None,
        })
    }
}

/// A relative selector, an argument of `:has()`: a complex selector whose
/// first compound is the anchor ([`SimpleSelector::RelativeAnchor`]), joined
/// to the written selector by the leading combinator (descendant if none
/// was written).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelativeSelector {
    selector: ComplexSelector,
}

impl RelativeSelector {
    /// The combinator between the anchor and the written selector.
    pub fn leading_combinator(&self) -> Combinator {
        self.selector.combinators[0]
    }

    /// The anchored selector: compound 0 is the anchor alone.
    pub fn selector(&self) -> &ComplexSelector {
        &self.selector
    }
}

/// The argument list of `:has()`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelativeSelectorList {
    selectors: Vec<RelativeSelector>,
    height: usize,
}

impl RelativeSelectorList {
    /// The relative selectors in source order (never empty).
    pub fn selectors(&self) -> &[RelativeSelector] {
        &self.selectors
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
    /// `:host` and `:host(<compound-selector>)` (CSS Scoping §3.2.1): the
    /// shadow host, seen from inside its shadow tree. The argument, when
    /// present, is a list of exactly one single-compound selector.
    Host(Option<Box<SelectorList>>),
    /// `:has(<relative-selector-list>)`.
    Has(RelativeSelectorList),
    /// An HTML form-control state.
    Form(FormState),
    /// `:lang()` with its language ranges as written (idents or strings).
    Lang(Vec<String>),
    /// `:dir()` with its argument ASCII-lowercased; only `ltr` and `rtl`
    /// can match.
    Dir(String),
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
    /// `::placeholder`.
    Placeholder,
    /// `::file-selector-button`.
    FileSelectorButton,
    /// `::backdrop`.
    Backdrop,
    /// `::selection` (a highlight pseudo-element).
    Selection,
    /// `::target-text` (a highlight pseudo-element).
    TargetText,
    /// `::spelling-error` (a highlight pseudo-element).
    SpellingError,
    /// `::grammar-error` (a highlight pseudo-element).
    GrammarError,
}

impl PseudoElement {
    fn name(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
            Self::FirstLine => "first-line",
            Self::FirstLetter => "first-letter",
            Self::Marker => "marker",
            Self::Placeholder => "placeholder",
            Self::FileSelectorButton => "file-selector-button",
            Self::Backdrop => "backdrop",
            Self::Selection => "selection",
            Self::TargetText => "target-text",
            Self::SpellingError => "spelling-error",
            Self::GrammarError => "grammar-error",
        }
    }

    fn from_name(name: &str, legacy_only: bool) -> Option<Self> {
        let pe = match name {
            "before" => Self::Before,
            "after" => Self::After,
            "first-line" => Self::FirstLine,
            "first-letter" => Self::FirstLetter,
            _ if legacy_only => return None,
            "marker" => Self::Marker,
            "placeholder" => Self::Placeholder,
            "file-selector-button" => Self::FileSelectorButton,
            "backdrop" => Self::Backdrop,
            "selection" => Self::Selection,
            "target-text" => Self::TargetText,
            "spelling-error" => Self::SpellingError,
            "grammar-error" => Self::GrammarError,
            _ => return None,
        };
        Some(pe)
    }

    /// Whether this is a highlight pseudo-element (CSS Pseudo-Elements
    /// Level 4 §3), which no pseudo-class may follow.
    pub fn is_highlight(self) -> bool {
        matches!(
            self,
            Self::Selection | Self::TargetText | Self::SpellingError | Self::GrammarError
        )
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
    /// The CSS Nesting selector `&`: the parent style rule's selectors
    /// (as `:is()`, pseudo-element selectors dropped), or `:scope` with zero
    /// specificity outside a nested rule.
    Nesting(Option<Arc<SelectorList>>),
    /// The `:has()` anchor element. Never written: it is the first compound
    /// of every [`RelativeSelector`] and serializes as nothing.
    RelativeAnchor,
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
    pseudo_element_states: Vec<PseudoClass>,
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

    /// User-action pseudo-classes written after the pseudo-element (such as
    /// `::before:hover`); each is [`PseudoClass::State`] or an `:is()`,
    /// `:where()` or `:not()` of compounds made only of those.
    pub fn pseudo_element_states(&self) -> &[PseudoClass] {
        &self.pseudo_element_states
    }

    /// Specificity per Selectors Level 4 §17.
    pub fn specificity(&self) -> Specificity {
        self.specificity
    }
}

/// A comma-separated list of complex selectors.
#[derive(Clone, Debug, Default)]
pub struct SelectorList {
    selectors: Vec<ComplexSelector>,
    /// How many nested selector lists (pseudo-class arguments and `&`
    /// parents) deep this list goes; bounds matching recursion.
    height: usize,
}

/// Comparison results keyed by the two lists' addresses.
type ListMemo = HashMap<(usize, usize), bool>;

thread_local! {
    /// Results of the list comparisons inside the outermost
    /// [`SelectorList::eq`] call, keyed by the two lists' addresses, plus that
    /// call's depth. Nested rules share their parent's list through `&`, so a
    /// sheet's selectors form a graph; comparing it as a tree costs
    /// exponential time in the nesting depth. The memo lives for one
    /// top-level comparison only, while every keyed list is borrowed.
    static LIST_EQ: RefCell<(usize, ListMemo)> =
        RefCell::new((0, HashMap::new()));
}

impl PartialEq for SelectorList {
    fn eq(&self, other: &Self) -> bool {
        if std::ptr::eq(self, other) {
            return true;
        }
        if self.height != other.height || self.selectors.len() != other.selectors.len() {
            return false;
        }
        let key = (self as *const Self as usize, other as *const Self as usize);
        let known = LIST_EQ.with(|memo| {
            let mut memo = memo.borrow_mut();
            memo.0 += 1;
            memo.1.get(&key).copied()
        });
        let result = known.unwrap_or_else(|| self.selectors == other.selectors);
        LIST_EQ.with(|memo| {
            let mut memo = memo.borrow_mut();
            memo.0 -= 1;
            if memo.0 == 0 {
                memo.1.clear();
            } else {
                memo.1.insert(key, result);
            }
        });
        result
    }
}

impl Eq for SelectorList {}

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
    Parser::new(namespaces, None).list(values, 0, true, false)
}

/// Parse the selector list of a scoped style rule or a scoping limit.
///
/// Relative entries are anchored to the scoping root with zero additional
/// specificity. An explicit `&` or `:scope` supplies its own anchor unless
/// the entry starts with a combinator.
pub fn parse_scoped_selector_list(
    values: &[Value],
    namespaces: &NamespaceMap,
    allow_pseudo_element: bool,
) -> Result<SelectorList, SelectorError> {
    let mut parser = Parser::new(namespaces, None);
    parser.scoped = true;
    parser.list(values, 0, allow_pseudo_element, false)
}

/// Parse the selector list of a style rule nested in a style rule whose
/// selectors are `parent` (CSS Nesting §2-3, Editor's Draft reviewed
/// 2026-11-08).
///
/// Each entry is a relative selector. An entry that starts with a combinator
/// or contains no `&` is made relative to the parent by prefixing `&` and the
/// combinator (a descendant combinator if none was written), so `> .a`
/// becomes `& > .a` and `.b` becomes `& .b`. Every `&` represents the
/// parent's selectors as `:is()` would, with pseudo-element selectors
/// dropped. One invalid entry invalidates the list.
///
/// ```
/// use std::sync::Arc;
/// use rustic_css::{CssParser, NamespaceMap, parse_nested_selector_list, parse_selector_list};
/// let parser = CssParser::default();
/// let namespaces = NamespaceMap::new();
/// let parent = parse_selector_list(&parser.component_values("ul, #menu").unwrap().value, &namespaces)
///     .unwrap();
/// let nested = parser.component_values("> li, &.open").unwrap().value;
/// let list = parse_nested_selector_list(&nested, &namespaces, &Arc::new(parent)).unwrap();
/// assert_eq!(list.to_string(), "& > li, &.open");
/// // `&` takes the parent's highest specificity, (1, 0, 0) from `#menu`.
/// assert_eq!(list.selectors()[0].specificity().ids, 1);
/// ```
pub fn parse_nested_selector_list(
    values: &[Value],
    namespaces: &NamespaceMap,
    parent: &Arc<SelectorList>,
) -> Result<SelectorList, SelectorError> {
    let parent = if parent.selectors.iter().any(|s| s.pseudo_element.is_some()) {
        let selectors: Vec<ComplexSelector> = parent
            .selectors
            .iter()
            .filter(|s| s.pseudo_element.is_none())
            .cloned()
            .collect();
        Arc::new(SelectorList {
            selectors,
            height: parent.height,
        })
    } else {
        Arc::clone(parent)
    };
    Parser::new(namespaces, Some(parent)).list(values, 0, true, false)
}

struct Parser<'a> {
    namespaces: &'a NamespaceMap,
    /// The parent rule's selectors, for `&` in a nested rule.
    parent: Option<Arc<SelectorList>>,
    scoped: bool,
    /// Whether the selector being parsed has used `&` so far.
    saw_nesting: Cell<bool>,
    /// Whether parsing is inside `:has()`, which may not nest.
    in_has: Cell<bool>,
}

/// The nesting height a list adds through one simple selector.
fn simple_height(simple: &SimpleSelector) -> usize {
    let list = |l: &SelectorList| l.height + 1;
    match simple {
        SimpleSelector::PseudoClass(class) => pseudo_class_height(class),
        SimpleSelector::Nesting(Some(parent)) => list(parent),
        _ => 0,
    }
}

fn pseudo_class_height(class: &PseudoClass) -> usize {
    match class {
        PseudoClass::Is(l) | PseudoClass::Where(l) | PseudoClass::Not(l) => l.height + 1,
        PseudoClass::Nth {
            nth: Nth { of: Some(l), .. },
            ..
        } => l.height + 1,
        PseudoClass::Has(l) => l.height + 1,
        PseudoClass::Host(Some(l)) => l.height + 1,
        _ => 0,
    }
}

fn complex_height(selector: &ComplexSelector) -> usize {
    selector
        .compounds
        .iter()
        .flat_map(|c| c.simple.iter())
        .map(simple_height)
        .chain(
            selector
                .pseudo_element_states
                .iter()
                .map(pseudo_class_height),
        )
        .max()
        .unwrap_or(0)
}

fn selector_uses_scope(selector: &ComplexSelector) -> bool {
    let uses_scope = |list: &SelectorList| list.selectors.iter().any(selector_uses_scope);
    selector
        .compounds
        .iter()
        .flat_map(|c| &c.simple)
        .any(|simple| match simple {
            SimpleSelector::Nesting(None) | SimpleSelector::PseudoClass(PseudoClass::Scope) => true,
            SimpleSelector::PseudoClass(
                PseudoClass::Is(list) | PseudoClass::Where(list) | PseudoClass::Not(list),
            ) => uses_scope(list),
            SimpleSelector::PseudoClass(PseudoClass::Nth { nth, .. }) => {
                nth.of.as_deref().is_some_and(uses_scope)
            }
            SimpleSelector::PseudoClass(PseudoClass::Host(list)) => {
                list.as_deref().is_some_and(uses_scope)
            }
            SimpleSelector::PseudoClass(PseudoClass::Has(list)) => list
                .selectors
                .iter()
                .any(|relative| selector_uses_scope(&relative.selector)),
            _ => false,
        })
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

impl<'a> Parser<'a> {
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
        // Only the entries of a nested rule's own list are relative.
        let nested_top = depth == 0 && (self.scoped || self.parent.is_some());
        for entry in split_commas(values) {
            let parsed = if nested_top {
                self.nested_entry(entry, allow_pseudo_element)
            } else {
                self.complex(entry, depth, allow_pseudo_element)
            };
            match parsed {
                Ok(selector) => selectors.push(selector),
                // A nesting overflow is a resource limit, never forgiven.
                Err(e) if forgiving && e.kind != SelectorErrorKind::NestingLimit => {}
                Err(e) => return Err(e),
            }
        }
        if selectors.is_empty() && !forgiving {
            return err(SelectorErrorKind::Empty, values, 0);
        }
        let height = selectors.iter().map(complex_height).max().unwrap_or(0);
        Ok(SelectorList { selectors, height })
    }

    fn new(namespaces: &'a NamespaceMap, parent: Option<Arc<SelectorList>>) -> Self {
        Self {
            namespaces,
            parent,
            scoped: false,
            saw_nesting: Cell::new(false),
            in_has: Cell::new(false),
        }
    }

    /// A leading `>`, `+` or `~` at `i` (after whitespace), and the index of
    /// the selector after it.
    fn leading_combinator(values: &[Value], i: usize) -> (Option<Combinator>, usize) {
        let combinator = match token_at(values, i) {
            Some(Token::Delim('>')) => Combinator::Child,
            Some(Token::Delim('+')) => Combinator::NextSibling,
            Some(Token::Delim('~')) => Combinator::SubsequentSibling,
            _ => return (None, i),
        };
        (Some(combinator), skip_whitespace(values, i + 1))
    }

    /// One entry of a nested rule's selector list (CSS Nesting §2.1).
    fn nested_entry(
        &self,
        entry: &[Value],
        allow_pseudo_element: bool,
    ) -> Result<ComplexSelector, SelectorError> {
        let start = skip_whitespace(entry, 0);
        let (leading, rest) = Self::leading_combinator(entry, start);
        if leading.is_some() && rest == entry.len() {
            return err(SelectorErrorKind::DanglingCombinator, entry, start);
        }
        self.saw_nesting.set(false);
        let selector = self.complex(&entry[rest..], 0, allow_pseudo_element)?;
        let has_anchor = if self.scoped {
            selector_uses_scope(&selector)
        } else {
            self.saw_nesting.get()
        };
        if leading.is_none() && has_anchor {
            return Ok(selector);
        }
        if self
            .parent
            .as_ref()
            .is_some_and(|parent| parent.height + 1 > MAX_SELECTOR_NESTING)
        {
            return err(SelectorErrorKind::NestingLimit, entry, start);
        }
        let specificity = self
            .parent
            .as_ref()
            .map_or(Specificity::default(), |parent| parent.max_specificity());
        let anchor = Compound {
            simple: vec![SimpleSelector::Nesting(self.parent.clone())],
        };
        Ok(prefix(
            anchor,
            leading.unwrap_or(Combinator::Descendant),
            specificity,
            selector,
        ))
    }

    /// `:has()` arguments: an unforgiving relative selector list.
    fn relative_list(
        &self,
        values: &[Value],
        depth: usize,
        span: SourceSpan,
    ) -> Result<RelativeSelectorList, SelectorError> {
        if depth > MAX_SELECTOR_NESTING {
            return err(SelectorErrorKind::NestingLimit, values, 0);
        }
        let mut selectors = Vec::new();
        for entry in split_commas(values) {
            let start = skip_whitespace(entry, 0);
            if start == entry.len() {
                return Err(SelectorError {
                    kind: SelectorErrorKind::Empty,
                    span,
                });
            }
            let (leading, rest) = Self::leading_combinator(entry, start);
            if rest == entry.len() {
                return err(SelectorErrorKind::DanglingCombinator, entry, start);
            }
            let selector = self.complex(&entry[rest..], depth, false)?;
            let anchor = Compound {
                simple: vec![SimpleSelector::RelativeAnchor],
            };
            selectors.push(RelativeSelector {
                selector: prefix(
                    anchor,
                    leading.unwrap_or(Combinator::Descendant),
                    Specificity::default(),
                    selector,
                ),
            });
        }
        let height = selectors
            .iter()
            .map(|r| complex_height(&r.selector))
            .max()
            .unwrap_or(0);
        Ok(RelativeSelectorList { selectors, height })
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
        let mut pseudo_element_states = Vec::new();
        let pseudo_element = loop {
            let (compound, compound_specificity, pseudo_element, mut next) =
                self.compound(values, i, depth, allow_pseudo_element)?;
            compounds.push(compound);
            specificity = specificity.add(compound_specificity);
            if let Some(pe) = pseudo_element {
                specificity = specificity.add(Specificity::TYPE);
                while token_at(values, next) == Some(&Token::Colon) {
                    if token_at(values, next + 1) == Some(&Token::Colon) {
                        return err(SelectorErrorKind::MisplacedPseudoElement, values, next);
                    }
                    let (class, class_specificity) = self.pseudo_class(values, next + 1, depth)?;
                    if pe.is_highlight() || !is_user_action_state(&class) {
                        return err(SelectorErrorKind::InvalidPseudoElementState, values, next);
                    }
                    specificity = specificity.add(class_specificity);
                    pseudo_element_states.push(class);
                    next += 2;
                }
            }
            let after_space = skip_whitespace(values, next);
            if pseudo_element.is_some() {
                if after_space != values.len() {
                    return err(SelectorErrorKind::MisplacedPseudoElement, values, next);
                }
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
            pseudo_element_states,
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
                // Without a default namespace `*|` means what no prefix
                // means, and CSSOM serializes it without one.
                None if self.namespaces.default_namespace().is_none() => {
                    NamespaceSelector::Implicit(None)
                }
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
                    self.saw_nesting.set(true);
                    let parent = self.parent.clone();
                    if let Some(parent) = &parent {
                        if depth + parent.height + 1 > MAX_SELECTOR_NESTING {
                            return err(SelectorErrorKind::NestingLimit, values, i);
                        }
                        specificity = specificity.add(parent.max_specificity());
                    }
                    i += 1;
                    SimpleSelector::Nesting(parent)
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
                "host" => PseudoClass::Host(None),
                other => match FormState::from_name(other) {
                    Some(state) => PseudoClass::Form(state),
                    None => return unsupported(),
                },
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
                    "host" => {
                        let invalid = SelectorError {
                            kind: SelectorErrorKind::InvalidHostArgument,
                            span: value.span,
                        };
                        let list = self.list(arguments, depth + 1, false, false).map_err(|e| {
                            if e.kind == SelectorErrorKind::NestingLimit {
                                e
                            } else {
                                invalid
                            }
                        })?;
                        let [only] = list.selectors() else {
                            return Err(invalid);
                        };
                        if only.compounds().len() != 1 {
                            return Err(invalid);
                        }
                        let s = Specificity::CLASS.add(list.max_specificity());
                        return Ok((PseudoClass::Host(Some(Box::new(list))), s));
                    }
                    "has" => {
                        if self.in_has.get() {
                            return unsupported();
                        }
                        self.in_has.set(true);
                        let list = self.relative_list(arguments, depth + 1, value.span);
                        self.in_has.set(false);
                        let list = list?;
                        let s = list
                            .selectors
                            .iter()
                            .map(|r| r.selector.specificity)
                            .max()
                            .unwrap_or_default();
                        return Ok((PseudoClass::Has(list), s));
                    }
                    "lang" => {
                        lang_arguments(arguments)
                            .map(PseudoClass::Lang)
                            .ok_or(SelectorError {
                                kind: SelectorErrorKind::UnexpectedToken,
                                span: value.span,
                            })?
                    }
                    "dir" => {
                        let words: Vec<&Value> =
                            arguments.iter().filter(|v| !v.is_whitespace()).collect();
                        match words.as_slice() {
                            [word] => match word.token() {
                                Some(Token::Ident(dir)) => {
                                    PseudoClass::Dir(dir.to_ascii_lowercase())
                                }
                                _ => return err(SelectorErrorKind::UnexpectedToken, values, i),
                            },
                            _ => return err(SelectorErrorKind::UnexpectedToken, values, i),
                        }
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

/// `anchor <combinator> selector`, with `specificity` added for the anchor.
fn prefix(
    anchor: Compound,
    combinator: Combinator,
    specificity: Specificity,
    mut selector: ComplexSelector,
) -> ComplexSelector {
    selector.compounds.insert(0, anchor);
    selector.combinators.insert(0, combinator);
    selector.specificity = selector.specificity.add(specificity);
    selector
}

/// Whether `class` may follow a tree-abiding pseudo-element: a user-action
/// state, or `:is()`/`:where()`/`:not()` of compounds of only those.
fn is_user_action_state(class: &PseudoClass) -> bool {
    match class {
        PseudoClass::State(state) => state.is_user_action(),
        PseudoClass::Is(list) | PseudoClass::Where(list) | PseudoClass::Not(list) => {
            list.selectors.iter().all(|s| {
                s.compounds.len() == 1
                    && s.compounds[0].simple.iter().all(|simple| {
                        matches!(simple, SimpleSelector::PseudoClass(c) if is_user_action_state(c))
                    })
            })
        }
        _ => false,
    }
}

/// `:lang()` arguments: a comma-separated list of idents or strings.
fn lang_arguments(arguments: &[Value]) -> Option<Vec<String>> {
    split_commas(arguments)
        .into_iter()
        .map(|entry| {
            let words: Vec<&Value> = entry.iter().filter(|v| !v.is_whitespace()).collect();
            match words.as_slice() {
                [word] => match word.token() {
                    Some(Token::Ident(range) | Token::String(range)) => Some(range.clone()),
                    _ => None,
                },
                _ => None,
            }
        })
        .collect()
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
                // `|` is the attribute default (no namespace): CSSOM omits it.
                if attribute.namespace != NamespaceSelector::None {
                    write_namespace(f, &attribute.namespace)?;
                }
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
            Self::Nesting(_) => f.write_char('&'),
            Self::RelativeAnchor => Ok(()),
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
            Self::Host(None) => f.write_str("host"),
            Self::Host(Some(list)) => write!(f, "host({list})"),
            Self::Has(list) => write!(f, "has({list})"),
            Self::Form(state) => f.write_str(state.name()),
            Self::Lang(ranges) => {
                f.write_str("lang(")?;
                for (index, range) in ranges.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    // CSSOM serializes each range as a string unless it is a
                    // valid identifier written as one; strings are always
                    // a fixed point.
                    serialize_string(f, range)?;
                }
                f.write_char(')')
            }
            Self::Dir(dir) => {
                f.write_str("dir(")?;
                serialize_identifier(f, dir)?;
                f.write_char(')')
            }
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

impl ComplexSelector {
    /// Serialize compounds from `first` on (1 skips a relative anchor).
    fn write_from(&self, f: &mut impl Write, first: usize) -> fmt::Result {
        for (index, compound) in self.compounds.iter().enumerate().skip(first) {
            if index > first {
                f.write_str(match self.combinators[index - 1] {
                    Combinator::Descendant => " ",
                    Combinator::Child => " > ",
                    Combinator::NextSibling => " + ",
                    Combinator::SubsequentSibling => " ~ ",
                })?;
            }
            // A lone unprefixed `*` before a pseudo-element adds nothing.
            let bare_universal = self.compounds.len() == 1
                && self.pseudo_element.is_some()
                && matches!(
                    compound.simple.as_slice(),
                    [SimpleSelector::Universal {
                        namespace: NamespaceSelector::Implicit(_)
                    }]
                );
            if !bare_universal {
                write!(f, "{compound}")?;
            }
        }
        if let Some(pe) = self.pseudo_element {
            write!(f, "::{}", pe.name())?;
        }
        for state in &self.pseudo_element_states {
            write!(f, "{state}")?;
        }
        Ok(())
    }
}

impl fmt::Display for ComplexSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_from(f, 0)
    }
}

impl fmt::Display for RelativeSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.leading_combinator() {
            Combinator::Descendant => "",
            Combinator::Child => "> ",
            Combinator::NextSibling => "+ ",
            Combinator::SubsequentSibling => "~ ",
        })?;
        self.selector.write_from(f, 1)
    }
}

impl fmt::Display for RelativeSelectorList {
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
