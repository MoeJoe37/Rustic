//! Stylesheets prepared for the cascade: style rules with parsed selectors
//! and typed declarations, the cascade layers they belong to, plus the
//! diagnostics for everything dropped.

use std::collections::HashMap;

use rustic_base::SourceSpan;
use rustic_css::{
    AtRule, BlockItem, CssError, CssParser, MediaQueryList, NamespaceMap, PropertyDeclaration,
    PropertyError, Rule, SelectorErrorKind, SelectorList, Token, Value, parse_block_contents,
    parse_declaration, parse_media_query_list, parse_selector_list,
};

use crate::{LayerName, LayerSegment};

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
#[derive(Clone, Debug, PartialEq)]
pub struct StyleRule {
    /// The rule's selectors.
    pub selectors: SelectorList,
    /// Typed longhand declarations, in source order (shorthands expanded).
    pub declarations: Vec<PropertyDeclaration>,
    /// Indices into [`StyleSheet::media`] of the media query lists the rule
    /// is nested in (outermost first); the rule applies only when all match.
    pub media: Vec<usize>,
    /// The rule's cascade layer, as an index into its sheet's
    /// [`StyleSheet::layers`]; `None` for an unlayered rule. An index past
    /// the end of the list is treated as unlayered.
    pub layer: Option<usize>,
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
    /// Every cascade layer the sheet declares, fully qualified, in order of
    /// first declaration (`@layer` statements and blocks alike). Layer
    /// order follows this list; style rules refer to it by index.
    pub layers: Vec<LayerName>,
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
    /// A nested style rule (CSS Nesting) is not implemented yet.
    UnsupportedNestedRule,
    /// An at-rule that is not implemented yet, such as `@import`.
    UnsupportedAtRule,
    /// A malformed or misplaced `@namespace` rule.
    InvalidNamespaceRule,
    /// An `@media` rule without a block.
    InvalidMediaRule,
    /// A malformed `@layer` rule, such as one naming a layer `initial`, or a
    /// block rule naming more than one layer.
    InvalidLayerRule,
    /// A declaration directly inside a grouping rule at the top level, such
    /// as `@media print { color: red }` or `@layer a { color: red }`.
    MisplacedDeclaration,
}

/// A dropped construct and its byte range in the stylesheet source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StyleDiagnostic {
    /// What was dropped and why.
    pub kind: StyleDiagnosticKind,
    /// Where it was.
    pub span: SourceSpan,
}

fn declarations(
    block: &[Value],
    diagnostics: &mut Vec<StyleDiagnostic>,
) -> Vec<PropertyDeclaration> {
    let mut out = Vec::new();
    for item in parse_block_contents(block, 0).value {
        match item {
            BlockItem::Declaration(declaration) => match parse_declaration(&declaration) {
                Ok(longhands) => out.extend(longhands),
                Err(error) => diagnostics.push(StyleDiagnostic {
                    kind: match error {
                        PropertyError::UnknownProperty => StyleDiagnosticKind::UnknownProperty,
                        PropertyError::InvalidValue => StyleDiagnosticKind::InvalidValue,
                    },
                    span: declaration.span,
                }),
            },
            BlockItem::Rule(rule) => diagnostics.push(StyleDiagnostic {
                kind: StyleDiagnosticKind::UnsupportedNestedRule,
                span: match &rule {
                    Rule::Qualified(q) => q.span,
                    Rule::At(a) => a.span,
                },
            }),
        }
    }
    out
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

/// One level of grouping rules being read: the remaining items, the
/// enclosing layer (an index into the sheet's layers) and the enclosing media
/// conditions (indices into the sheet's media lists, outermost first).
struct Frame {
    items: std::vec::IntoIter<BlockItem>,
    layer: Option<usize>,
    media: Vec<usize>,
}

/// Builds a [`StyleSheet`] from rules, tracking layers and diagnostics.
struct Builder {
    sheet: StyleSheet,
    diagnostics: Vec<StyleDiagnostic>,
    namespaces: NamespaceMap,
    /// Whether an `@namespace` rule may still appear.
    namespaces_allowed: bool,
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

    /// `@media`: the frame for its block, with its query list added.
    fn media_rule(&mut self, at: AtRule, layer: Option<usize>, media: &[usize]) -> Option<Frame> {
        let Some(block) = at.block else {
            self.diagnose(StyleDiagnosticKind::InvalidMediaRule, at.span);
            return None;
        };
        let index = self.sheet.media.len();
        self.sheet.media.push(parse_media_query_list(&at.prelude));
        let mut nested = media.to_vec();
        nested.push(index);
        Some(Frame {
            items: parse_block_contents(&block, 0).value.into_iter(),
            layer,
            media: nested,
        })
    }

    /// `@layer`: declare the named layers, or return the frame for a block.
    fn layer_rule(&mut self, at: AtRule, parent: Option<usize>, media: &[usize]) -> Option<Frame> {
        let names = layer_names(&at.prelude);
        match (names, at.block) {
            (Some(names), None) if !names.is_empty() => {
                for name in names {
                    self.declare_layer(parent, name);
                }
                None
            }
            (Some(mut names), Some(block)) if names.len() <= 1 => {
                let segments = names.pop().unwrap_or_else(|| {
                    self.anonymous_layers = self.anonymous_layers.saturating_add(1);
                    vec![LayerSegment::Anonymous(self.anonymous_layers)]
                });
                let layer = self.declare_layer(parent, segments);
                Some(Frame {
                    items: parse_block_contents(&block, 0).value.into_iter(),
                    layer: Some(layer),
                    media: media.to_vec(),
                })
            }
            _ => {
                self.diagnose(StyleDiagnosticKind::InvalidLayerRule, at.span);
                None
            }
        }
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
    /// layers; both nest to any depth the parser allows. Other at-rules and
    /// nested style rules are reported as unsupported.
    pub fn parse(
        parser: &CssParser,
        source: &str,
        origin: Origin,
    ) -> Result<(Self, Vec<StyleDiagnostic>), CssError> {
        let sheet = parser.stylesheet(source)?.value;
        let mut builder = Builder {
            sheet: StyleSheet {
                origin,
                ..Self::default()
            },
            diagnostics: Vec::new(),
            namespaces: NamespaceMap::new(),
            namespaces_allowed: true,
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
        }];
        while let Some(frame) = stack.last_mut() {
            let Some(item) = frame.items.next() else {
                stack.pop();
                continue;
            };
            let layer = frame.layer;
            let media = frame.media.clone();
            let top_level = stack.len() == 1;
            let rule = match item {
                BlockItem::Rule(rule) => rule,
                BlockItem::Declaration(declaration) => {
                    builder.diagnose(StyleDiagnosticKind::MisplacedDeclaration, declaration.span);
                    continue;
                }
            };
            match rule {
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
                    if let Some(frame) = builder.layer_rule(at, layer, &media) {
                        stack.push(frame);
                    }
                }
                Rule::At(at) if at.name.eq_ignore_ascii_case("media") => {
                    builder.namespaces_allowed = false;
                    if let Some(frame) = builder.media_rule(at, layer, &media) {
                        stack.push(frame);
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
                    match parse_selector_list(&q.prelude, &builder.namespaces) {
                        Ok(selectors) => {
                            let declarations = declarations(&q.block, &mut builder.diagnostics);
                            builder.sheet.rules.push(StyleRule {
                                selectors,
                                declarations,
                                media,
                                layer,
                            });
                        }
                        Err(error) => builder
                            .diagnose(StyleDiagnosticKind::InvalidSelector(error.kind), q.span),
                    }
                }
            }
        }
        Ok((builder.sheet, builder.diagnostics))
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

    /// Whether `rule`'s media conditions all hold, given each of this
    /// sheet's media lists' results.
    pub(crate) fn rule_applies(rule: &StyleRule, media_results: &[bool]) -> bool {
        rule.media
            .iter()
            .all(|&i| media_results.get(i).copied().unwrap_or(false))
    }
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
