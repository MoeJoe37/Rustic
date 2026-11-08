//! Stylesheets prepared for the cascade: style rules with parsed selectors
//! and typed declarations, plus the diagnostics for everything dropped.

use rustic_base::SourceSpan;
use rustic_css::{
    BlockItem, CssError, CssParser, NamespaceMap, PropertyDeclaration, PropertyError, Rule,
    SelectorErrorKind, SelectorList, Token, parse_block_contents, parse_declaration,
    parse_selector_list,
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
}

/// A stylesheet ready for the cascade.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleSheet {
    /// The sheet's origin.
    pub origin: Origin,
    /// Style rules in source order.
    pub rules: Vec<StyleRule>,
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
    /// An at-rule that is not implemented yet, such as `@media` or `@import`.
    UnsupportedAtRule,
    /// A malformed or misplaced `@namespace` rule.
    InvalidNamespaceRule,
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

impl StyleSheet {
    /// Parse a stylesheet and prepare it for the cascade.
    ///
    /// Invalid style rules and declarations are dropped with diagnostics, as
    /// CSS requires; only resource limits ([`CssError`]) fail the whole sheet.
    /// `@namespace` rules before the first style rule build the namespace
    /// map; other at-rules and nested rules are reported as unsupported.
    pub fn parse(
        parser: &CssParser,
        source: &str,
        origin: Origin,
    ) -> Result<(Self, Vec<StyleDiagnostic>), CssError> {
        let sheet = parser.stylesheet(source)?.value;
        let mut diagnostics = Vec::new();
        let mut namespaces = NamespaceMap::new();
        let mut rules = Vec::new();
        let mut namespaces_allowed = true;
        for rule in sheet.rules {
            match rule {
                Rule::At(at) if at.name.eq_ignore_ascii_case("namespace") => {
                    match namespace_rule(&at.prelude)
                        .filter(|_| namespaces_allowed && at.block.is_none())
                    {
                        Some((Some(prefix), url)) => {
                            namespaces = namespaces.with_prefix(prefix, url)
                        }
                        Some((None, url)) => namespaces = namespaces.with_default(url),
                        None => diagnostics.push(StyleDiagnostic {
                            kind: StyleDiagnosticKind::InvalidNamespaceRule,
                            span: at.span,
                        }),
                    }
                }
                Rule::At(at) => {
                    if !at.name.eq_ignore_ascii_case("charset")
                        && !at.name.eq_ignore_ascii_case("import")
                    {
                        namespaces_allowed = false;
                    }
                    diagnostics.push(StyleDiagnostic {
                        kind: StyleDiagnosticKind::UnsupportedAtRule,
                        span: at.span,
                    });
                }
                Rule::Qualified(q) => {
                    namespaces_allowed = false;
                    match parse_selector_list(&q.prelude, &namespaces) {
                        Ok(selectors) => rules.push(StyleRule {
                            selectors,
                            declarations: declarations(&q.block, &mut diagnostics),
                        }),
                        Err(error) => diagnostics.push(StyleDiagnostic {
                            kind: StyleDiagnosticKind::InvalidSelector(error.kind),
                            span: q.span,
                        }),
                    }
                }
            }
        }
        Ok((Self { origin, rules }, diagnostics))
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
