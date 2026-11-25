//! Stylesheets prepared for the cascade: style rules with parsed selectors
//! and typed declarations, the cascade layers they belong to, plus the
//! diagnostics for everything dropped.

use std::collections::HashMap;
use std::sync::Arc;

use rustic_base::SourceSpan;
use rustic_css::{
    AtRule, BlockItem, ComponentValue, ContainerQueryList, CssError, CssParser, CustomValue,
    Declaration, DeclaredValue, MediaQueryList, NamespaceMap, PropertyDeclaration, PropertyError,
    PropertySyntax, Rule, SelectorErrorKind, SelectorList, Token, UnparsedValue, Value,
    parse_block_contents, parse_container_query_list, parse_declaration, parse_media_query_list,
    parse_nested_selector_list, parse_scoped_selector_list, parse_selector_list,
};

use rustic_url::Url;

use crate::{LayerName, LayerSegment, PropertyRegistration, ScopeRule};

/// Where a stylesheet comes from (CSS Cascade Level 5 §6.2).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Origin {
    /// The user agent's default styles.
    UserAgent,
    /// The user's own styles.
    User,
    /// The page's styles, including `style` attributes.
    #[default]
    Author,
}

/// A style rule: a selector list and its typed declarations in order.
///
/// A nested style rule appears as its own `StyleRule` after its parent, with
/// `&` resolved to the parent's selectors; declarations that follow a nested
/// rule (or sit in a nested `@media`) become another `StyleRule` with the
/// parent's selectors.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleRule {
    /// The rule's selectors.
    pub selectors: SelectorList,
    /// Typed longhand declarations, in source order (shorthands expanded).
    pub declarations: Vec<PropertyDeclaration>,
    /// Indices into [`StyleSheet::media`] of the conditional rules the rule
    /// is nested in (outermost first); the rule applies only when all match.
    /// An index listed in [`StyleSheet::containers`] is an `@container`
    /// condition (its media list is empty, so it always matches as media).
    pub media: Vec<usize>,
    /// The rule's cascade layer, as an index into its sheet's
    /// [`StyleSheet::layers`]; `None` for an unlayered rule. An index past
    /// the end of the list is treated as unlayered.
    pub layer: Option<usize>,
    /// The explicit `@scope` this rule belongs to, if any.
    pub scope: Option<Arc<ScopeRule>>,
}

/// One keyframe block of an `@keyframes` rule.
#[derive(Clone, Debug, PartialEq)]
pub struct Keyframe {
    /// Its keyframe selectors, as offsets in `0..=1` (`from` is 0, `to`
    /// is 1), or timeline range names with offsets for scroll-driven
    /// animations.
    pub selectors: Vec<KeyframeSelector>,
    /// Its declarations (`!important` ones are dropped).
    pub declarations: Vec<PropertyDeclaration>,
}

/// A keyframe selector.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyframeSelector {
    /// A percentage of the whole animation, as a fraction.
    Offset(f64),
    /// `<timeline-range-name> <percentage>` (Scroll-driven Animations §4.3).
    Range(Arc<str>, f64),
}

/// An `@keyframes` rule.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyframesRule {
    /// The keyframes name (case-sensitive).
    pub name: Arc<str>,
    /// The keyframe blocks in source order.
    pub keyframes: Vec<Keyframe>,
    /// The rule's cascade layer in its sheet.
    pub layer: Option<usize>,
    /// The conditional rules it sits in (indices into [`StyleSheet::media`]).
    pub media: Vec<usize>,
}

/// `<keyframe-selector>#`: `from`, `to`, percentages in `0%..=100%`, or a
/// timeline range name and a percentage.
fn keyframe_selectors(prelude: &[Value]) -> Option<Vec<KeyframeSelector>> {
    let mut out = Vec::new();
    for part in trim_whitespace(prelude).split(|v| v.token() == Some(&Token::Comma)) {
        let values: Vec<&Value> = significant(part).collect();
        let percent = |v: &Value| match v.token() {
            Some(Token::Percentage(n)) if (0.0..=100.0).contains(&n.value) => Some(n.value / 100.0),
            _ => None,
        };
        out.push(match values.as_slice() {
            [one] => match one.token() {
                Some(Token::Ident(name)) if name.eq_ignore_ascii_case("from") => {
                    KeyframeSelector::Offset(0.0)
                }
                Some(Token::Ident(name)) if name.eq_ignore_ascii_case("to") => {
                    KeyframeSelector::Offset(1.0)
                }
                _ => KeyframeSelector::Offset(percent(one)?),
            },
            [range, offset] => {
                let Some(Token::Ident(name)) = range.token() else {
                    return None;
                };
                let name = name.to_ascii_lowercase();
                if ![
                    "cover",
                    "contain",
                    "entry",
                    "exit",
                    "entry-crossing",
                    "exit-crossing",
                    "scroll",
                ]
                .contains(&name.as_str())
                {
                    return None;
                }
                let offset = match offset.token() {
                    Some(Token::Percentage(n)) => n.value / 100.0,
                    _ => return None,
                };
                KeyframeSelector::Range(name.into(), offset)
            }
            _ => return None,
        });
    }
    (!out.is_empty()).then_some(out)
}

/// A stylesheet ready for the cascade.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleSheet {
    /// The sheet's origin.
    pub origin: Origin,
    /// Style rules in source order, with the rules of grouping rules such as
    /// `@media` and `@layer` flattened in place.
    pub rules: Vec<StyleRule>,
    /// The media query lists that [`StyleRule::media`] refers to: one per
    /// `@media` rule, plus any added for the whole sheet (such as a `<style>`
    /// element's `media` attribute).
    pub media: Vec<MediaQueryList>,
    /// `@container` conditions, as (index into [`StyleSheet::media`], query
    /// list) pairs in increasing index order (they never apply to
    /// `@keyframes`, which need no element).
    pub containers: Vec<(usize, ContainerQueryList)>,
    /// Every cascade layer the sheet declares, fully qualified, in order of
    /// first declaration (`@layer` statements and blocks alike). Layer
    /// order follows this list; style rules refer to it by index.
    pub layers: Vec<LayerName>,
    /// Custom properties registered by valid `@property` rules, in source
    /// order (a later registration of a name wins).
    pub properties: Vec<PropertyRegistration>,
    /// Valid `@keyframes` rules in source order, each with its cascade
    /// layer (an index into [`StyleSheet::layers`]).
    pub keyframes: Vec<Arc<KeyframesRule>>,
    /// The URL that relative `url()` values resolve against: the sheet's
    /// own URL for an external or imported sheet, the document base URL
    /// for a `<style>` element. Set it with [`StyleSheet::set_base_url`].
    pub base_url: Option<Url>,
}

/// Why part of a stylesheet was dropped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StyleDiagnosticKind {
    /// A style rule's selector list is invalid, so the whole rule is dropped.
    InvalidSelector(SelectorErrorKind),
    /// A declaration names a property that is not supported.
    UnknownProperty,
    /// A declaration's value does not match its property's grammar.
    InvalidValue,
    /// A nested at-rule other than `@media` inside a style rule, or a nested
    /// rule in a `style` attribute, which is not implemented yet.
    UnsupportedNestedRule,
    /// An at-rule that is not implemented yet, such as `@import`.
    UnsupportedAtRule,
    /// A malformed or misplaced `@namespace` rule.
    InvalidNamespaceRule,
    /// An `@media` rule without a block.
    InvalidMediaRule,
    /// An `@container` rule without a block or with an invalid prelude.
    InvalidContainerRule,
    /// An explicit `@scope` with an invalid root, limit or missing block.
    InvalidScopeRule,
    /// A malformed `@layer` rule, such as one naming a layer `initial`, or a
    /// block rule naming more than one layer.
    InvalidLayerRule,
    /// A declaration directly inside a grouping rule at the top level, such
    /// as `@media print { color: red }` or `@layer a { color: red }`.
    MisplacedDeclaration,
    /// A malformed `@import` rule, one inside another rule, or one after a
    /// rule other than `@charset`, `@layer` statements and other `@import`s.
    InvalidImportRule,
    /// An `@keyframes` rule with an invalid name or no block, or one
    /// inside a style rule. Invalid keyframe selectors drop only their
    /// keyframe, with this diagnostic too.
    InvalidKeyframesRule,
    /// An `@property` rule that is malformed (a missing or invalid
    /// `syntax`, `inherits` or `initial-value`, or an unsupported syntax),
    /// or that is inside a conditional rule or a style rule.
    InvalidPropertyRule,
}

/// A dropped construct and its byte range in the stylesheet source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StyleDiagnostic {
    /// What was dropped and why.
    pub kind: StyleDiagnosticKind,
    /// Where it was.
    pub span: SourceSpan,
}

fn rule_span(rule: &Rule) -> SourceSpan {
    match rule {
        Rule::Qualified(q) => q.span,
        Rule::At(a) => a.span,
    }
}

/// Type one declaration, appending its longhands or a diagnostic.
fn declaration(
    declaration: &Declaration,
    out: &mut Vec<PropertyDeclaration>,
    diagnostics: &mut Vec<StyleDiagnostic>,
) {
    match parse_declaration(declaration) {
        Ok(longhands) => out.extend(longhands),
        Err(error) => diagnostics.push(StyleDiagnostic {
            kind: match error {
                PropertyError::UnknownProperty => StyleDiagnosticKind::UnknownProperty,
                PropertyError::InvalidValue => StyleDiagnosticKind::InvalidValue,
            },
            span: declaration.span,
        }),
    }
}

/// A block's declarations, with any nested rule dropped (style attributes).
fn declarations(
    block: &[Value],
    diagnostics: &mut Vec<StyleDiagnostic>,
) -> Vec<PropertyDeclaration> {
    let mut out = Vec::new();
    for item in parse_block_contents(block, 0).value {
        match item {
            BlockItem::Declaration(d) => declaration(&d, &mut out, diagnostics),
            BlockItem::Rule(rule) => diagnostics.push(StyleDiagnostic {
                kind: StyleDiagnosticKind::UnsupportedNestedRule,
                span: rule_span(&rule),
            }),
        }
    }
    out
}

/// One style rule (or `@media` block nested in one) being expanded: its
/// selectors, media conditions, block items and the declarations gathered
/// since the last nested rule.
struct NestingFrame {
    selectors: Arc<SelectorList>,
    media: Vec<usize>,
    items: std::vec::IntoIter<BlockItem>,
    pending: Vec<PropertyDeclaration>,
    emitted: bool,
}

impl NestingFrame {
    fn new(selectors: Arc<SelectorList>, media: Vec<usize>, block: &[Value]) -> Self {
        Self {
            selectors,
            media,
            items: parse_block_contents(block, 0).value.into_iter(),
            pending: Vec::new(),
            emitted: false,
        }
    }

    /// Emit the rule itself the first time, then a nested declarations rule
    /// for each later run of declarations.
    fn flush(
        &mut self,
        rules: &mut Vec<StyleRule>,
        layer: Option<usize>,
        scope: &Option<Arc<ScopeRule>>,
    ) {
        if !self.pending.is_empty() || !self.emitted {
            rules.push(StyleRule {
                selectors: SelectorList::clone(&self.selectors),
                declarations: std::mem::take(&mut self.pending),
                media: self.media.clone(),
                layer,
                scope: scope.clone(),
            });
            self.emitted = true;
        }
    }
}

/// `@namespace <prefix>? [<string> | <url>];`
fn namespace_rule(prelude: &[Value]) -> Option<(Option<String>, String)> {
    let parts: Vec<&Token> = prelude
        .iter()
        .filter(|v| !v.is_whitespace())
        .map(|v| v.token())
        .collect::<Option<_>>()?;
    let url = |t: &Token| match t {
        Token::String(s) | Token::Url(s) => Some(s.clone()),
        _ => None,
    };
    match parts.as_slice() {
        [only] => Some((None, url(only)?)),
        [Token::Ident(prefix), target] => Some((Some(prefix.clone()), url(target)?)),
        _ => None,
    }
}

/// Whether `name` is reserved in `<layer-name>`: the CSS-wide keywords.
fn reserved_layer_ident(name: &str) -> bool {
    [
        "initial",
        "inherit",
        "unset",
        "revert",
        "revert-layer",
        "revert-rule",
    ]
    .iter()
    .any(|k| name.eq_ignore_ascii_case(k))
}

/// `<layer-name> = <ident> [ '.' <ident> ]*`, with no whitespace inside.
fn layer_name(values: &[Value]) -> Option<Vec<LayerSegment>> {
    let mut segments = Vec::new();
    let mut want_ident = true;
    for value in values {
        match (value.token()?, want_ident) {
            (Token::Ident(name), true) if !reserved_layer_ident(name) => {
                segments.push(LayerSegment::Named(name.clone()));
            }
            (Token::Delim('.'), false) => {}
            _ => return None,
        }
        want_ident = !want_ident;
    }
    (!want_ident).then_some(segments)
}

fn trim_whitespace(values: &[Value]) -> &[Value] {
    let start = values.iter().position(|v| !v.is_whitespace());
    let end = values.iter().rposition(|v| !v.is_whitespace());
    match (start, end) {
        (Some(start), Some(end)) => &values[start..=end],
        _ => &[],
    }
}

/// The comma-separated layer names of an `@layer` prelude; an empty list
/// for an empty prelude, `None` if any name is invalid.
fn layer_names(prelude: &[Value]) -> Option<Vec<Vec<LayerSegment>>> {
    let prelude = trim_whitespace(prelude);
    if prelude.is_empty() {
        return Some(Vec::new());
    }
    prelude
        .split(|v| v.token() == Some(&Token::Comma))
        .map(|part| layer_name(trim_whitespace(part)))
        .collect()
}

/// An `@import` rule (CSS Cascade Level 5 §2.1), not yet fetched.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportRule {
    /// The URL as written, before resolution against the sheet's URL.
    pub url: String,
    /// Whether a `layer` or `layer(...)` condition is present.
    pub layer: bool,
    /// The `scope(...)` the imported sheet's rules are scoped to.
    pub scope: Option<Arc<ScopeRule>>,
    /// Whether a `supports(...)` condition is present.
    pub supports: bool,
    /// The media query list, or `None` when there is none (all media).
    pub media: Option<MediaQueryList>,
    /// The whole rule in the stylesheet source.
    pub span: SourceSpan,
}

/// `@import [<url> | <string>] [layer | layer(...)]? [supports(...)]?
/// <media-query-list>?;`
fn import_rule(at: &AtRule) -> Option<ImportRule> {
    if at.block.is_some() {
        return None;
    }
    let mut rest = at.prelude.as_slice();
    fn skip_whitespace(rest: &[Value]) -> &[Value] {
        let start = rest
            .iter()
            .position(|v| !v.is_whitespace())
            .unwrap_or(rest.len());
        &rest[start..]
    }
    rest = skip_whitespace(rest);
    let url = match &rest.first()?.value {
        ComponentValue::Token(Token::String(s) | Token::Url(s)) => s.clone(),
        ComponentValue::Function { name, arguments } if name.eq_ignore_ascii_case("url") => {
            let mut args = arguments.iter().filter(|v| !v.is_whitespace());
            match (args.next().and_then(|v| v.token()), args.next()) {
                (Some(Token::String(s)), None) => s.clone(),
                _ => return None,
            }
        }
        _ => return None,
    };
    rest = &rest[1..];
    let is_function = |value: &Value, wanted: &str| {
        matches!(&value.value, ComponentValue::Function { name, .. }
            if name.eq_ignore_ascii_case(wanted))
    };
    rest = skip_whitespace(rest);
    let layer = rest.first().is_some_and(|v| {
        is_function(v, "layer")
            || matches!(&v.value, ComponentValue::Token(Token::Ident(n))
                if n.eq_ignore_ascii_case("layer"))
    });
    if layer {
        rest = &rest[1..];
    }
    rest = skip_whitespace(rest);
    let mut scope = None;
    if let Some(ComponentValue::Function { name, arguments }) = rest.first().map(|v| &v.value) {
        if name.eq_ignore_ascii_case("scope") {
            scope = Some(Arc::new(import_scope(arguments)?));
            rest = &rest[1..];
        }
    }
    rest = skip_whitespace(rest);
    let supports = rest.first().is_some_and(|v| is_function(v, "supports"));
    if supports {
        rest = &rest[1..];
    }
    rest = skip_whitespace(rest);
    let media = (!rest.is_empty()).then(|| parse_media_query_list(rest));
    Some(ImportRule {
        url,
        layer,
        scope,
        supports,
        media,
        span: at.span,
    })
}

/// The argument of `@import ... scope(...)`: `<scope-start>` alone, or
/// `(<scope-start>)? [to (<scope-end>)]?` as in `@scope`.
fn import_scope(arguments: &[Value]) -> Option<ScopeRule> {
    let arguments = trim_whitespace(arguments);
    let namespaces = NamespaceMap::new();
    let boundaries = arguments.first().is_none_or(|first| {
        matches!(
            &first.value,
            ComponentValue::Block {
                kind: rustic_css::BlockKind::Paren,
                ..
            }
        ) || matches!(first.token(), Some(Token::Ident(s)) if s.eq_ignore_ascii_case("to"))
    });
    if boundaries {
        return ScopeRule::parse(arguments, &namespaces, None, None);
    }
    let wrapped = Value {
        value: ComponentValue::Block {
            kind: rustic_css::BlockKind::Paren,
            contents: arguments.to_vec(),
        },
        ..arguments[0].clone()
    };
    ScopeRule::parse(std::slice::from_ref(&wrapped), &namespaces, None, None)
}

/// One level of grouping rules being read: the remaining items, the
/// enclosing layer (an index into the sheet's layers) and the enclosing media
/// conditions (indices into the sheet's media lists, outermost first).
struct Frame {
    items: std::vec::IntoIter<BlockItem>,
    layer: Option<usize>,
    media: Vec<usize>,
    scope: Option<Arc<ScopeRule>>,
    /// Declarations directly in an `@scope` block, waiting for the next
    /// rule or the end of the block (scoped declarations).
    pending: Vec<PropertyDeclaration>,
}

impl Frame {
    fn new(
        block: &[Value],
        layer: Option<usize>,
        media: Vec<usize>,
        scope: Option<Arc<ScopeRule>>,
    ) -> Self {
        Self {
            items: parse_block_contents(block, 0).value.into_iter(),
            layer,
            media,
            scope,
            pending: Vec::new(),
        }
    }

    /// Emit pending scoped declarations as a `:where(:scope)` rule.
    fn flush(&mut self, rules: &mut Vec<StyleRule>) {
        if self.pending.is_empty() {
            return;
        }
        rules.push(StyleRule {
            selectors: scope_self(),
            declarations: std::mem::take(&mut self.pending),
            media: self.media.clone(),
            layer: self.layer,
            scope: self.scope.clone(),
        });
    }
}

/// `:where(:scope)`, the selector of declarations directly in `@scope`
/// (CSS Cascade Level 6 §3.5.1): the scoping root, with no specificity.
fn scope_self() -> SelectorList {
    static SELECTOR: std::sync::OnceLock<SelectorList> = std::sync::OnceLock::new();
    SELECTOR
        .get_or_init(|| {
            let parsed = CssParser::default()
                .stylesheet(":where(:scope) {}")
                .expect("a fixed stylesheet parses")
                .value;
            let Some(Rule::Qualified(rule)) = parsed.rules.first() else {
                unreachable!("one qualified rule");
            };
            parse_scoped_selector_list(&rule.prelude, &NamespaceMap::new(), false)
                .expect("a fixed selector parses")
        })
        .clone()
}

/// Builds a [`StyleSheet`] from rules, tracking layers and diagnostics.
struct Builder {
    sheet: StyleSheet,
    diagnostics: Vec<StyleDiagnostic>,
    namespaces: NamespaceMap,
    /// Whether an `@namespace` rule may still appear.
    namespaces_allowed: bool,
    /// Collected `@import` rules; `None` reports them as unsupported.
    imports: Option<Vec<ImportRule>>,
    /// Whether an `@import` rule may still appear.
    imports_allowed: bool,
    anonymous_layers: u32,
    /// Index of each layer in `sheet.layers`.
    layer_index: HashMap<LayerName, usize>,
}

impl Builder {
    fn diagnose(&mut self, kind: StyleDiagnosticKind, span: SourceSpan) {
        self.diagnostics.push(StyleDiagnostic { kind, span });
    }

    /// The index of layer `parent` + `segments`, declaring it if new.
    fn declare_layer(&mut self, parent: Option<usize>, segments: Vec<LayerSegment>) -> usize {
        let mut full = parent.map_or_else(Vec::new, |p| self.sheet.layers[p].segments().to_vec());
        full.extend(segments);
        let name = LayerName::new(full).expect("layer names have a segment");
        let next = self.sheet.layers.len();
        let index = *self.layer_index.entry(name.clone()).or_insert(next);
        if index == next {
            self.sheet.layers.push(name);
        }
        index
    }

    /// The condition of an `@media` or `@container` rule with a block, as a
    /// new index into the sheet's media lists, and the block; `None` (with a
    /// diagnostic) for a rule without a block or an invalid container query.
    fn condition(&mut self, at: AtRule) -> Option<(usize, Vec<Value>)> {
        let container = at.name.eq_ignore_ascii_case("container");
        let kind = if container {
            StyleDiagnosticKind::InvalidContainerRule
        } else {
            StyleDiagnosticKind::InvalidMediaRule
        };
        let query = if container {
            parse_container_query_list(&at.prelude).map(Some)
        } else {
            Some(None)
        };
        let (Some(block), Some(query)) = (at.block, query) else {
            self.diagnose(kind, at.span);
            return None;
        };
        let index = self.sheet.media.len();
        match query {
            Some(query) => {
                self.sheet.media.push(MediaQueryList {
                    queries: Vec::new(),
                });
                self.sheet.containers.push((index, query));
            }
            None => self.sheet.media.push(parse_media_query_list(&at.prelude)),
        }
        Some((index, block))
    }

    /// `@media` or `@container`: the frame for its block, with its
    /// condition added.
    fn media_rule(&mut self, at: AtRule, parent: &Frame) -> Option<Frame> {
        let (index, block) = self.condition(at)?;
        let mut nested = parent.media.clone();
        nested.push(index);
        Some(Frame::new(
            &block,
            parent.layer,
            nested,
            parent.scope.clone(),
        ))
    }

    /// Append a style rule and its nested rules (CSS Nesting §3, Editor's
    /// Draft reviewed 2026-11-08) in the order the cascade sees them: the
    /// rule with the declarations before its first nested rule, then each
    /// nested rule (expanded the same way, depth-first), with every later run
    /// of declarations as a nested declarations rule that has the parent's
    /// selectors. A nested `@media` adds its condition and keeps the
    /// parent's selectors for its declarations and relative rules. All of
    /// them stay in `layer`. The expansion is iterative; block nesting is
    /// bounded by the parser.
    fn expand(
        &mut self,
        selectors: SelectorList,
        layer: Option<usize>,
        media: Vec<usize>,
        scope: Option<Arc<ScopeRule>>,
        block: &[Value],
    ) {
        let mut stack = vec![NestingFrame::new(Arc::new(selectors), media, block)];
        while let Some(frame) = stack.last_mut() {
            let Some(item) = frame.items.next() else {
                frame.flush(&mut self.sheet.rules, layer, &scope);
                stack.pop();
                continue;
            };
            match item {
                BlockItem::Declaration(d) => {
                    declaration(&d, &mut frame.pending, &mut self.diagnostics);
                }
                BlockItem::Rule(Rule::Qualified(q)) => {
                    match parse_nested_selector_list(&q.prelude, &self.namespaces, &frame.selectors)
                    {
                        Ok(nested) => {
                            frame.flush(&mut self.sheet.rules, layer, &scope);
                            let media = frame.media.clone();
                            stack.push(NestingFrame::new(Arc::new(nested), media, &q.block));
                        }
                        Err(error) => {
                            self.diagnose(StyleDiagnosticKind::InvalidSelector(error.kind), q.span);
                        }
                    }
                }
                BlockItem::Rule(Rule::At(at))
                    if at.name.eq_ignore_ascii_case("media")
                        || at.name.eq_ignore_ascii_case("container") =>
                {
                    let Some((index, block)) = self.condition(at) else {
                        continue;
                    };
                    frame.flush(&mut self.sheet.rules, layer, &scope);
                    let mut media = frame.media.clone();
                    media.push(index);
                    let mut child = NestingFrame::new(Arc::clone(&frame.selectors), media, &block);
                    // Only declarations inside it make a rule.
                    child.emitted = true;
                    stack.push(child);
                }
                BlockItem::Rule(Rule::At(at)) if at.name.eq_ignore_ascii_case("scope") => {
                    frame.flush(&mut self.sheet.rules, layer, &scope);
                    let selectors = Arc::clone(&frame.selectors);
                    let media = frame.media.clone();
                    self.nested_scope_rule(at, &selectors, layer, media, scope.clone());
                }
                BlockItem::Rule(rule) => {
                    self.diagnose(StyleDiagnosticKind::UnsupportedNestedRule, rule_span(&rule));
                }
            }
        }
    }

    /// `@layer`: declare the named layers, or return the frame for a block.
    fn layer_rule(&mut self, at: AtRule, parent: &Frame) -> Option<Frame> {
        let names = layer_names(&at.prelude);
        match (names, at.block) {
            (Some(names), None) if !names.is_empty() => {
                for name in names {
                    self.declare_layer(parent.layer, name);
                }
                None
            }
            (Some(mut names), Some(block)) if names.len() <= 1 => {
                let segments = names.pop().unwrap_or_else(|| {
                    self.anonymous_layers = self.anonymous_layers.saturating_add(1);
                    vec![LayerSegment::Anonymous(self.anonymous_layers)]
                });
                let layer = self.declare_layer(parent.layer, segments);
                Some(Frame::new(
                    &block,
                    Some(layer),
                    parent.media.clone(),
                    parent.scope.clone(),
                ))
            }
            _ => {
                self.diagnose(StyleDiagnosticKind::InvalidLayerRule, at.span);
                None
            }
        }
    }

    /// `@keyframes <keyframes-name> { <keyframe-block>* }` (CSS
    /// Animations 1 §3, editor's draft reviewed 2026-11-25).
    fn keyframes_rule(&mut self, at: AtRule, layer: Option<usize>, media: Vec<usize>) {
        let prelude: Vec<&Value> = significant(&at.prelude).collect();
        let name = match prelude.as_slice() {
            [only] => match only.token() {
                Some(Token::Ident(name)) => {
                    let lower = name.to_ascii_lowercase();
                    let reserved = [
                        "none",
                        "default",
                        "initial",
                        "inherit",
                        "unset",
                        "revert",
                        "revert-layer",
                    ];
                    (!reserved.contains(&lower.as_str())).then(|| name.clone())
                }
                Some(Token::String(name)) => Some(name.clone()),
                _ => None,
            },
            _ => None,
        };
        let (Some(name), Some(block)) = (name, at.block) else {
            self.diagnose(StyleDiagnosticKind::InvalidKeyframesRule, at.span);
            return;
        };
        let mut keyframes = Vec::new();
        for item in parse_block_contents(&block, 0).value {
            let rule = match item {
                BlockItem::Rule(Rule::Qualified(rule)) => rule,
                BlockItem::Rule(rule) => {
                    self.diagnose(StyleDiagnosticKind::InvalidKeyframesRule, rule_span(&rule));
                    continue;
                }
                BlockItem::Declaration(d) => {
                    self.diagnose(StyleDiagnosticKind::MisplacedDeclaration, d.span);
                    continue;
                }
            };
            let Some(selectors) = keyframe_selectors(&rule.prelude) else {
                self.diagnose(StyleDiagnosticKind::InvalidKeyframesRule, rule.span);
                continue;
            };
            let mut declarations = Vec::new();
            for item in parse_block_contents(&rule.block, 0).value {
                match item {
                    // `!important` keyframe declarations are ignored.
                    BlockItem::Declaration(d) if d.important => {}
                    BlockItem::Declaration(d) => {
                        declaration(&d, &mut declarations, &mut self.diagnostics);
                    }
                    BlockItem::Rule(rule) => {
                        self.diagnose(StyleDiagnosticKind::UnsupportedNestedRule, rule_span(&rule))
                    }
                }
            }
            keyframes.push(Keyframe {
                selectors,
                declarations,
            });
        }
        self.sheet.keyframes.push(Arc::new(KeyframesRule {
            name: name.into(),
            keyframes,
            layer,
            media,
        }));
    }

    /// `@scope`, at the top level or in a grouping rule (possibly in
    /// another `@scope`): the frame for its block.
    fn scope_rule(&mut self, at: AtRule, parent: &Frame) -> Option<Frame> {
        let prelude = trim_whitespace(&at.prelude);
        let scope = ScopeRule::parse(prelude, &self.namespaces, parent.scope.clone(), None);
        let (Some(block), Some(scope)) = (at.block, scope) else {
            self.diagnose(StyleDiagnosticKind::InvalidScopeRule, at.span);
            return None;
        };
        Some(Frame::new(
            &block,
            parent.layer,
            parent.media.clone(),
            Some(Arc::new(scope)),
        ))
    }

    /// `@scope` nested in a style rule whose selectors are `nesting`: its
    /// declarations apply to the scoping roots and its style rules are
    /// scoped, each expanded like any nested rule.
    fn nested_scope_rule(
        &mut self,
        at: AtRule,
        nesting: &SelectorList,
        layer: Option<usize>,
        media: Vec<usize>,
        outer: Option<Arc<ScopeRule>>,
    ) {
        let prelude = trim_whitespace(&at.prelude);
        let scope = ScopeRule::parse(prelude, &self.namespaces, outer, Some(nesting));
        let (Some(block), Some(scope)) = (at.block, scope) else {
            self.diagnose(StyleDiagnosticKind::InvalidScopeRule, at.span);
            return;
        };
        let mut frame = Frame::new(&block, layer, media, Some(Arc::new(scope)));
        while let Some(item) = frame.items.next() {
            match item {
                BlockItem::Declaration(d) => {
                    declaration(&d, &mut frame.pending, &mut self.diagnostics);
                }
                BlockItem::Rule(Rule::Qualified(q)) => {
                    frame.flush(&mut self.sheet.rules);
                    match parse_scoped_selector_list(&q.prelude, &self.namespaces, true) {
                        Ok(selectors) => self.expand(
                            selectors,
                            frame.layer,
                            frame.media.clone(),
                            frame.scope.clone(),
                            &q.block,
                        ),
                        Err(error) => {
                            self.diagnose(StyleDiagnosticKind::InvalidSelector(error.kind), q.span)
                        }
                    }
                }
                BlockItem::Rule(rule) => {
                    self.diagnose(StyleDiagnosticKind::UnsupportedNestedRule, rule_span(&rule));
                }
            }
        }
        frame.flush(&mut self.sheet.rules);
    }
}

impl StyleSheet {
    /// Parse a stylesheet and prepare it for the cascade.
    ///
    /// Invalid style rules and declarations are dropped with diagnostics, as
    /// CSS requires; only resource limits ([`CssError`]) fail the whole sheet.
    /// `@namespace` rules before the first style rule build the namespace
    /// map. `@media` rules attach their media conditions to their style
    /// rules, and `@layer` statements and blocks assign rules to cascade
    /// layers; both nest to any depth the parser allows. Nested style rules
    /// expand after their parent (see [`StyleRule`]), and `@media` may also
    /// nest inside them. Other at-rules are reported as unsupported.
    pub fn parse(
        parser: &CssParser,
        source: &str,
        origin: Origin,
    ) -> Result<(Self, Vec<StyleDiagnostic>), CssError> {
        Self::parse_inner(parser, source, origin, false)
            .map(|(sheet, _, diagnostics)| (sheet, diagnostics))
    }

    /// Like [`StyleSheet::parse`], but also returns the sheet's valid
    /// `@import` rules in order, for the caller to fetch. Imported rules come
    /// before the sheet's own rules in the cascade. Misplaced or malformed
    /// `@import` rules are dropped with
    /// [`StyleDiagnosticKind::InvalidImportRule`].
    pub fn parse_with_imports(
        parser: &CssParser,
        source: &str,
        origin: Origin,
    ) -> Result<(Self, Vec<ImportRule>, Vec<StyleDiagnostic>), CssError> {
        Self::parse_inner(parser, source, origin, true)
    }

    fn parse_inner(
        parser: &CssParser,
        source: &str,
        origin: Origin,
        collect_imports: bool,
    ) -> Result<(Self, Vec<ImportRule>, Vec<StyleDiagnostic>), CssError> {
        let sheet = parser.stylesheet(source)?.value;
        let mut builder = Builder {
            sheet: StyleSheet {
                origin,
                ..Self::default()
            },
            diagnostics: Vec::new(),
            namespaces: NamespaceMap::new(),
            namespaces_allowed: true,
            imports: collect_imports.then(Vec::new),
            imports_allowed: true,
            anonymous_layers: 0,
            layer_index: HashMap::new(),
        };
        // An explicit stack of grouping rules: block nesting is bounded by
        // the parser, but the walk stays iterative regardless.
        let top: Vec<BlockItem> = sheet.rules.into_iter().map(BlockItem::Rule).collect();
        let mut stack = vec![Frame {
            items: top.into_iter(),
            layer: None,
            media: Vec::new(),
            scope: None,
            pending: Vec::new(),
        }];
        while !stack.is_empty() {
            let top_level = stack.len() == 1;
            let frame = stack.last_mut().expect("nonempty stack");
            let Some(item) = frame.items.next() else {
                frame.flush(&mut builder.sheet.rules);
                stack.pop();
                continue;
            };
            let rule = match item {
                BlockItem::Rule(rule) => rule,
                // Declarations directly in `@scope` apply to its roots.
                BlockItem::Declaration(d) if frame.scope.is_some() => {
                    declaration(&d, &mut frame.pending, &mut builder.diagnostics);
                    continue;
                }
                BlockItem::Declaration(declaration) => {
                    builder.diagnose(StyleDiagnosticKind::MisplacedDeclaration, declaration.span);
                    continue;
                }
            };
            frame.flush(&mut builder.sheet.rules);
            let layer_statement = matches!(&rule, Rule::At(at)
                if at.name.eq_ignore_ascii_case("layer") && at.block.is_none());
            let keeps_imports = matches!(&rule, Rule::At(at)
                if at.name.eq_ignore_ascii_case("charset") || at.name.eq_ignore_ascii_case("import"));
            if !(top_level && (layer_statement || keeps_imports)) {
                builder.imports_allowed = false;
            }
            match rule {
                Rule::At(at)
                    if at.name.eq_ignore_ascii_case("import") && builder.imports.is_some() =>
                {
                    let import = import_rule(&at).filter(|_| top_level && builder.imports_allowed);
                    match (import, builder.imports.as_mut()) {
                        (Some(import), Some(imports)) => imports.push(import),
                        _ => builder.diagnose(StyleDiagnosticKind::InvalidImportRule, at.span),
                    }
                }
                Rule::At(at) if at.name.eq_ignore_ascii_case("namespace") => {
                    match namespace_rule(&at.prelude)
                        .filter(|_| top_level && builder.namespaces_allowed && at.block.is_none())
                    {
                        Some((Some(prefix), url)) => {
                            builder.namespaces =
                                std::mem::take(&mut builder.namespaces).with_prefix(prefix, url)
                        }
                        Some((None, url)) => {
                            builder.namespaces =
                                std::mem::take(&mut builder.namespaces).with_default(url)
                        }
                        None => {
                            builder.diagnose(StyleDiagnosticKind::InvalidNamespaceRule, at.span)
                        }
                    }
                }
                Rule::At(at) if at.name.eq_ignore_ascii_case("layer") => {
                    // Layer statements may precede `@namespace`; blocks may not.
                    if at.block.is_some() {
                        builder.namespaces_allowed = false;
                    }
                    if let Some(frame) = builder.layer_rule(at, frame) {
                        stack.push(frame);
                    }
                }
                Rule::At(at)
                    if at.name.eq_ignore_ascii_case("media")
                        || at.name.eq_ignore_ascii_case("container") =>
                {
                    builder.namespaces_allowed = false;
                    if let Some(frame) = builder.media_rule(at, frame) {
                        stack.push(frame);
                    }
                }
                Rule::At(at) if at.name.eq_ignore_ascii_case("scope") => {
                    builder.namespaces_allowed = false;
                    if let Some(frame) = builder.scope_rule(at, frame) {
                        stack.push(frame);
                    }
                }
                Rule::At(at)
                    if at.name.eq_ignore_ascii_case("keyframes")
                        || at.name.eq_ignore_ascii_case("-webkit-keyframes") =>
                {
                    builder.namespaces_allowed = false;
                    let layer = frame.layer;
                    let media = frame.media.clone();
                    builder.keyframes_rule(at, layer, media);
                }
                Rule::At(at) if at.name.eq_ignore_ascii_case("property") => {
                    builder.namespaces_allowed = false;
                    // Top level or in `@layer` blocks only: a registration
                    // is global, so it cannot depend on a condition.
                    match property_rule(&at).filter(|_| frame.media.is_empty()) {
                        Some(registration) => builder.sheet.properties.push(registration),
                        None => builder.diagnose(StyleDiagnosticKind::InvalidPropertyRule, at.span),
                    }
                }
                Rule::At(at) => {
                    if !at.name.eq_ignore_ascii_case("charset")
                        && !at.name.eq_ignore_ascii_case("import")
                    {
                        builder.namespaces_allowed = false;
                    }
                    builder.diagnose(StyleDiagnosticKind::UnsupportedAtRule, at.span);
                }
                Rule::Qualified(q) => {
                    builder.namespaces_allowed = false;
                    let parsed = if frame.scope.is_some() {
                        parse_scoped_selector_list(&q.prelude, &builder.namespaces, true)
                    } else {
                        parse_selector_list(&q.prelude, &builder.namespaces)
                    };
                    match parsed {
                        Ok(selectors) => builder.expand(
                            selectors,
                            frame.layer,
                            frame.media.clone(),
                            frame.scope.clone(),
                            &q.block,
                        ),
                        Err(error) => builder
                            .diagnose(StyleDiagnosticKind::InvalidSelector(error.kind), q.span),
                    }
                }
            }
        }
        let imports = builder.imports.unwrap_or_default();
        Ok((builder.sheet, imports, builder.diagnostics))
    }

    /// Record the sheet's base URL and resolve every `url()` in its
    /// declarations against it (CSS Values 4 §4.5.1). Values that contain
    /// `var()` resolve after substitution, against the same base.
    pub fn set_base_url(&mut self, base: Option<Url>) {
        if let Some(base) = &base {
            for rule in &mut self.rules {
                resolve_declaration_urls(&mut rule.declarations, base);
            }
            for rule in &mut self.keyframes {
                for keyframe in &mut Arc::make_mut(rule).keyframes {
                    resolve_declaration_urls(&mut keyframe.declarations, base);
                }
            }
        }
        self.base_url = base;
    }

    /// Root the sheet's implicit `@scope` rules (those without a start) at
    /// `owner`: the owner node's parent element, or the shadow host for a
    /// sheet in a shadow tree (CSS Cascade Level 6 §3.5). `None` roots them
    /// at the tree's root element.
    pub fn set_scope_owner(&mut self, owner: Option<rustic_dom::NodeId>) {
        let mut done: HashMap<*const ScopeRule, Arc<ScopeRule>> = HashMap::new();
        for rule in &mut self.rules {
            if let Some(scope) = &rule.scope {
                let key = Arc::as_ptr(scope);
                let updated = done
                    .entry(key)
                    .or_insert_with(|| Arc::new(scope.with_owner(owner)));
                rule.scope = Some(Arc::clone(updated));
            }
        }
    }

    /// Put every rule inside `outer`, as `@import ... scope(...)` does.
    pub fn scope_within(&mut self, outer: &Arc<ScopeRule>) {
        let mut done: HashMap<*const ScopeRule, Arc<ScopeRule>> = HashMap::new();
        for rule in &mut self.rules {
            rule.scope = Some(match &rule.scope {
                Some(scope) => Arc::clone(
                    done.entry(Arc::as_ptr(scope))
                        .or_insert_with(|| Arc::new(scope.within(outer))),
                ),
                None => Arc::clone(outer),
            });
        }
    }

    /// Make every rule also depend on `media` (for example a `<style>`
    /// element's `media` attribute).
    pub fn restrict_to(&mut self, media: MediaQueryList) {
        let index = self.media.len();
        self.media.push(media);
        for rule in &mut self.rules {
            rule.media.insert(0, index);
        }
    }

    /// The `@container` query list at media index `index`, if it is one.
    pub fn container_at(&self, index: usize) -> Option<&ContainerQueryList> {
        self.containers
            .binary_search_by_key(&index, |(i, _)| *i)
            .ok()
            .map(|i| &self.containers[i].1)
    }

    /// Whether `rule`'s media conditions all hold, given each of this
    /// sheet's media lists' results.
    pub(crate) fn rule_applies(rule: &StyleRule, media_results: &[bool]) -> bool {
        rule.media
            .iter()
            .all(|&i| media_results.get(i).copied().unwrap_or(false))
    }
}

/// The values without whitespace tokens.
fn significant(values: &[Value]) -> impl Iterator<Item = &Value> {
    values
        .iter()
        .filter(|v| !matches!(v.token(), Some(Token::Whitespace)))
}

/// A registration from an `@property` rule (CSS Properties and Values API
/// Level 1 §3), or `None` if the rule is invalid. Unknown descriptors are
/// ignored; for a repeated descriptor the last one counts.
fn property_rule(at: &AtRule) -> Option<PropertyRegistration> {
    let mut prelude = significant(&at.prelude);
    let name = match prelude.next()?.token()? {
        Token::Ident(name) if rustic_css::is_custom_property_name(name) => name.clone(),
        _ => return None,
    };
    if prelude.next().is_some() {
        return None;
    }
    let mut syntax = None;
    let mut inherits = None;
    let mut initial: Option<Vec<Value>> = None;
    for item in parse_block_contents(at.block.as_deref()?, 0).value {
        let BlockItem::Declaration(d) = item else {
            continue;
        };
        if d.important {
            return None;
        }
        let name = d.name.to_ascii_lowercase();
        match name.as_str() {
            "syntax" => {
                let mut values = significant(&d.value);
                syntax = match (values.next().and_then(|v| v.token()), values.next()) {
                    (Some(Token::String(s)), None) => Some(PropertySyntax::parse(s)?),
                    _ => return None,
                };
            }
            "inherits" => {
                let mut values = significant(&d.value);
                inherits = match (values.next().and_then(|v| v.token()), values.next()) {
                    (Some(Token::Ident(i)), None) if i.eq_ignore_ascii_case("true") => Some(true),
                    (Some(Token::Ident(i)), None) if i.eq_ignore_ascii_case("false") => Some(false),
                    _ => return None,
                };
            }
            "initial-value" => initial = Some(d.value),
            _ => {}
        }
    }
    let syntax = syntax?;
    let inherits = inherits?;
    let initial = match initial.map(|v| CustomValue::new(&v)) {
        None => None,
        Some(None) => return None,
        Some(Some(value)) if value.has_references() => return None,
        Some(Some(value)) if value.values().is_empty() => None,
        Some(Some(value)) => Some(value),
    };
    let initial = match (&syntax, initial) {
        (PropertySyntax::Universal, initial) => initial,
        (_, None) => return None,
        (syntax, Some(value)) => {
            let parsed = syntax.parse_value(&value)?;
            if !parsed.is_computationally_independent() {
                return None;
            }
            Some(CustomValue::new(
                &CssParser::default()
                    .component_values(&parsed.to_string())
                    .ok()?
                    .value,
            )?)
        }
    };
    Some(PropertyRegistration {
        name,
        syntax,
        inherits,
        initial,
    })
}

/// Parse a `style` attribute's value as declarations (CSS Style Attributes).
///
/// Nested rules and invalid declarations are dropped with diagnostics.
pub fn parse_style_attribute(
    parser: &CssParser,
    source: &str,
) -> Result<(Vec<PropertyDeclaration>, Vec<StyleDiagnostic>), CssError> {
    let values = parser.component_values(source)?.value;
    let mut diagnostics = Vec::new();
    let declarations = declarations(&values, &mut diagnostics);
    Ok((declarations, diagnostics))
}

/// Resolve a `url()` value against `base`: `None` (keep it as written) for
/// an empty or fragment-only URL, which refer to the current document and
/// never fetch, and for a URL that does not parse. CSS parses URLs as UTF-8
/// whatever the sheet's encoding.
pub(crate) fn resolve_css_url(url: &str, base: &Url) -> Option<String> {
    if url.is_empty() || url.starts_with('#') {
        return None;
    }
    Url::parse(url, Some(base)).ok().map(|url| url.serialize())
}

/// Resolve the `url()` values of `declarations` (a style rule's or a `style`
/// attribute's) against `base`, and record `base` on values that contain
/// `var()` so they resolve after substitution. The longhands of one
/// shorthand keep sharing one unparsed value.
pub fn resolve_declaration_urls(declarations: &mut [PropertyDeclaration], base: &Url) {
    let serialized: Arc<str> = base.serialize().into();
    let mut last: Option<(*const UnparsedValue, Arc<UnparsedValue>)> = None;
    for declaration in declarations {
        match &mut declaration.value {
            DeclaredValue::Value(value) => value.resolve_urls(&|url| resolve_css_url(url, base)),
            DeclaredValue::Unparsed(unparsed) => {
                let original = Arc::as_ptr(unparsed);
                if let Some((pointer, shared)) = &last {
                    if *pointer == original {
                        *unparsed = shared.clone();
                        continue;
                    }
                }
                let mut value = (**unparsed).clone();
                value.set_url_base(Some(serialized.clone()));
                *unparsed = Arc::new(value);
                last = Some((original, unparsed.clone()));
            }
            DeclaredValue::CssWide(_) | DeclaredValue::Custom(_) => {}
        }
    }
}
