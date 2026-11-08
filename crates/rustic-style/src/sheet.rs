//! Stylesheets prepared for the cascade: style rules with parsed selectors
//! and typed declarations, plus the diagnostics for everything dropped.

use rustic_base::SourceSpan;
use rustic_css::{
    AtRule, BlockItem, CssError, CssParser, MediaQueryList, NamespaceMap, PropertyDeclaration,
    PropertyError, QualifiedRule, Rule, SelectorErrorKind, SelectorList, Token,
    parse_block_contents, parse_declaration, parse_media_query_list, parse_selector_list,
};

/// Where a stylesheet comes from (CSS Cascade Level 5 §6.2).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Origin {
    /// The user agent's default styles.
    UserAgent,
    /// The user's own styles.
    User,
    /// The page's styles, including `style` attributes.
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
}

/// A stylesheet ready for the cascade.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleSheet {
    /// The sheet's origin.
    pub origin: Origin,
    /// Style rules in source order, with the rules of conditional groups
    /// such as `@media` flattened in place.
    pub rules: Vec<StyleRule>,
    /// The media query lists that [`StyleRule::media`] refers to: one per
    /// `@media` rule, plus any added for the whole sheet (such as a `<style>`
    /// element's `media` attribute).
    pub media: Vec<MediaQueryList>,
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
    /// A declaration directly inside a conditional group rule at the top
    /// level, such as `@media print { color: red }`.
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
    block: &[rustic_css::Value],
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
fn namespace_rule(prelude: &[rustic_css::Value]) -> Option<(Option<String>, String)> {
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

/// Builds a sheet's flat rule list.
struct Builder<'d> {
    namespaces: NamespaceMap,
    namespaces_allowed: bool,
    rules: Vec<StyleRule>,
    media: Vec<MediaQueryList>,
    diagnostics: &'d mut Vec<StyleDiagnostic>,
}

impl Builder<'_> {
    fn report(&mut self, kind: StyleDiagnosticKind, span: SourceSpan) {
        self.diagnostics.push(StyleDiagnostic { kind, span });
    }

    /// Handle one rule; `conditions` are the enclosing media list indices.
    fn rule(&mut self, rule: Rule, conditions: &[usize]) {
        match rule {
            Rule::Qualified(q) => self.style_rule(q, conditions),
            Rule::At(at) if at.name.eq_ignore_ascii_case("namespace") => {
                self.namespace(at, conditions.is_empty());
            }
            Rule::At(at) if at.name.eq_ignore_ascii_case("media") => {
                self.namespaces_allowed = false;
                self.media_rule(at, conditions);
            }
            Rule::At(at) => {
                if !at.name.eq_ignore_ascii_case("charset")
                    && !at.name.eq_ignore_ascii_case("import")
                {
                    self.namespaces_allowed = false;
                }
                self.report(StyleDiagnosticKind::UnsupportedAtRule, at.span);
            }
        }
    }

    fn namespace(&mut self, at: AtRule, top_level: bool) {
        match namespace_rule(&at.prelude)
            .filter(|_| top_level && self.namespaces_allowed && at.block.is_none())
        {
            Some((Some(prefix), url)) => {
                self.namespaces = std::mem::take(&mut self.namespaces).with_prefix(prefix, url);
            }
            Some((None, url)) => {
                self.namespaces = std::mem::take(&mut self.namespaces).with_default(url);
            }
            None => self.report(StyleDiagnosticKind::InvalidNamespaceRule, at.span),
        }
    }

    fn media_rule(&mut self, at: AtRule, conditions: &[usize]) {
        let Some(block) = at.block else {
            self.report(StyleDiagnosticKind::InvalidMediaRule, at.span);
            return;
        };
        let index = self.media.len();
        self.media.push(parse_media_query_list(&at.prelude));
        let mut nested = conditions.to_vec();
        nested.push(index);
        for item in parse_block_contents(&block, 0).value {
            match item {
                BlockItem::Rule(rule) => self.rule(rule, &nested),
                BlockItem::Declaration(declaration) => {
                    self.report(StyleDiagnosticKind::MisplacedDeclaration, declaration.span);
                }
            }
        }
    }

    fn style_rule(&mut self, q: QualifiedRule, conditions: &[usize]) {
        self.namespaces_allowed = false;
        match parse_selector_list(&q.prelude, &self.namespaces) {
            Ok(selectors) => {
                let declarations = declarations(&q.block, self.diagnostics);
                self.rules.push(StyleRule {
                    selectors,
                    declarations,
                    media: conditions.to_vec(),
                });
            }
            Err(error) => self.report(StyleDiagnosticKind::InvalidSelector(error.kind), q.span),
        }
    }
}

impl StyleSheet {
    /// Parse a stylesheet and prepare it for the cascade.
    ///
    /// Invalid style rules and declarations are dropped with diagnostics, as
    /// CSS requires; only resource limits ([`CssError`]) fail the whole sheet.
    /// `@namespace` rules before the first style rule build the namespace
    /// map, and `@media` rules (nested to any depth) contribute their style
    /// rules with the media conditions attached; other at-rules and nested
    /// style rules are reported as unsupported.
    pub fn parse(
        parser: &CssParser,
        source: &str,
        origin: Origin,
    ) -> Result<(Self, Vec<StyleDiagnostic>), CssError> {
        let sheet = parser.stylesheet(source)?.value;
        let mut diagnostics = Vec::new();
        let mut builder = Builder {
            namespaces: NamespaceMap::new(),
            namespaces_allowed: true,
            rules: Vec::new(),
            media: Vec::new(),
            diagnostics: &mut diagnostics,
        };
        for rule in sheet.rules {
            builder.rule(rule, &[]);
        }
        let (rules, media) = (builder.rules, builder.media);
        Ok((
            Self {
                origin,
                rules,
                media,
            },
            diagnostics,
        ))
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
