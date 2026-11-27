//! A read-only rule tree for the CSS Object Model (CSSOM §6.4): the rules
//! of a stylesheet as written, with the fields script reads from
//! `CSSStyleRule`, `CSSScopeRule`, `CSSMediaRule` and the rest. Text
//! fields are the source text of each part with whitespace collapsed, not
//! the canonical CSSOM serialization (TD-204).

use rustic_base::SourceSpan;
use rustic_css::{
    AtRule, BlockItem, BlockKind, ComponentValue, CssParser, Rule, Value, parse_block_contents,
};

/// What kind of rule a [`CssomRule`] is.
#[derive(Clone, Debug, PartialEq)]
pub enum CssomRuleKind {
    /// A style rule (`CSSStyleRule`, type 1).
    Style {
        /// `selectorText`.
        selector: String,
    },
    /// `@import` (`CSSImportRule`, type 3).
    Import {
        /// `href`.
        href: String,
        /// `layerName`: `None` without `layer`, `Some("")` for an
        /// anonymous layer.
        layer: Option<String>,
        /// `supportsText`.
        supports: Option<String>,
        /// `media.mediaText`.
        media: String,
    },
    /// `@media` (`CSSMediaRule`, type 4).
    Media {
        /// `conditionText` and `media.mediaText`.
        condition: String,
    },
    /// `@container` (`CSSContainerRule`).
    Container {
        /// `conditionText`.
        condition: String,
    },
    /// `@supports` (`CSSSupportsRule`, type 12).
    Supports {
        /// `conditionText`.
        condition: String,
    },
    /// `@keyframes` (`CSSKeyframesRule`, type 7).
    Keyframes {
        /// `name`.
        name: String,
    },
    /// A keyframe (`CSSKeyframeRule`, type 8).
    Keyframe {
        /// `keyText`.
        key: String,
    },
    /// `@layer` with a block (`CSSLayerBlockRule`).
    LayerBlock {
        /// `name` (empty for an anonymous layer).
        name: String,
    },
    /// `@layer a, b;` (`CSSLayerStatementRule`).
    LayerStatement {
        /// `nameList`.
        names: Vec<String>,
    },
    /// `@scope` (`CSSScopeRule`).
    Scope {
        /// `start`, or `None` for an implicit scope.
        start: Option<String>,
        /// `end`.
        end: Option<String>,
    },
    /// Declarations directly in a grouping rule (`CSSNestedDeclarations`).
    NestedDeclarations,
    /// `@namespace` (`CSSNamespaceRule`, type 10).
    Namespace {
        /// `prefix`.
        prefix: String,
        /// `namespaceURI`.
        uri: String,
    },
    /// `@font-face` (type 5).
    FontFace,
    /// `@property` (`CSSPropertyRule`).
    Property {
        /// `name`.
        name: String,
    },
    /// `@page` (type 6).
    Page,
    /// Any other at-rule Rustic does not model; script sees a plain
    /// `CSSRule`.
    Other,
}

impl CssomRuleKind {
    /// The legacy `CSSRule.type` constant (0 for newer rule types).
    pub fn legacy_type(&self) -> u16 {
        match self {
            Self::Style { .. } => 1,
            Self::Import { .. } => 3,
            Self::Media { .. } => 4,
            Self::FontFace => 5,
            Self::Page => 6,
            Self::Keyframes { .. } => 7,
            Self::Keyframe { .. } => 8,
            Self::Namespace { .. } => 10,
            Self::Supports { .. } => 12,
            _ => 0,
        }
    }

    /// The interface name script sees.
    pub fn interface(&self) -> &'static str {
        match self {
            Self::Style { .. } => "CSSStyleRule",
            Self::Import { .. } => "CSSImportRule",
            Self::Media { .. } => "CSSMediaRule",
            Self::Container { .. } => "CSSContainerRule",
            Self::Supports { .. } => "CSSSupportsRule",
            Self::Keyframes { .. } => "CSSKeyframesRule",
            Self::Keyframe { .. } => "CSSKeyframeRule",
            Self::LayerBlock { .. } => "CSSLayerBlockRule",
            Self::LayerStatement { .. } => "CSSLayerStatementRule",
            Self::Scope { .. } => "CSSScopeRule",
            Self::NestedDeclarations => "CSSNestedDeclarations",
            Self::Namespace { .. } => "CSSNamespaceRule",
            Self::FontFace => "CSSFontFaceRule",
            Self::Property { .. } => "CSSPropertyRule",
            Self::Page => "CSSPageRule",
            Self::Other => "CSSRule",
        }
    }
}

/// One declaration of a rule's `style`.
#[derive(Clone, Debug, PartialEq)]
pub struct CssomDeclaration {
    /// The property name (lowercased unless custom).
    pub name: String,
    /// The value text.
    pub value: String,
    /// `!important`.
    pub important: bool,
}

/// A rule in a [`CssomSheet`].
#[derive(Clone, Debug, PartialEq)]
pub struct CssomRule {
    /// Its kind and kind-specific fields.
    pub kind: CssomRuleKind,
    /// `cssText`.
    pub text: String,
    /// The rule's declarations (`style`), for rules that have them.
    pub declarations: Vec<CssomDeclaration>,
    /// Child rules (`cssRules`), for grouping rules.
    pub rules: Vec<CssomRule>,
}

/// The rule tree of one stylesheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CssomSheet {
    /// Top-level rules.
    pub rules: Vec<CssomRule>,
}

/// Most rules kept for one sheet's CSSOM.
const MAX_RULES: usize = 65_536;

impl CssomSheet {
    /// Build the rule tree of `source`. A sheet past the parser's limits
    /// reads as empty.
    pub fn parse(source: &str) -> Self {
        let parser = CssParser::default();
        let Ok(parsed) = parser.stylesheet(source) else {
            return Self::default();
        };
        let mut budget = MAX_RULES;
        let items: Vec<BlockItem> = parsed
            .value
            .rules
            .into_iter()
            .map(BlockItem::Rule)
            .collect();
        Self {
            rules: rules(source, items, false, &mut budget),
        }
    }
}

fn text(source: &str, span: SourceSpan) -> String {
    collapse(span.slice_str(source).unwrap_or(""))
}

fn collapse(text: &str) -> String {
    text.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
}

fn values_text(source: &str, values: &[Value]) -> String {
    let significant: Vec<&Value> = values.iter().filter(|v| !v.is_whitespace()).collect();
    match (significant.first(), significant.last()) {
        (Some(first), Some(last)) => {
            match SourceSpan::new(span_start(source, first.span), span_end(source, last.span)) {
                Ok(span) => text(source, span),
                Err(_) => String::new(),
            }
        }
        _ => String::new(),
    }
}

fn span_start(source: &str, span: SourceSpan) -> usize {
    // A span's text begins at its start; find it through the slice.
    span.slice_str(source).map_or(0, |slice| {
        slice.as_ptr() as usize - source.as_ptr() as usize
    })
}

fn span_end(source: &str, span: SourceSpan) -> usize {
    span.slice_str(source).map_or(0, |slice| {
        slice.as_ptr() as usize - source.as_ptr() as usize + slice.len()
    })
}

fn declarations(source: &str, items: &[BlockItem]) -> Vec<CssomDeclaration> {
    items
        .iter()
        .filter_map(|item| match item {
            BlockItem::Declaration(d) => Some(CssomDeclaration {
                name: if d.name.starts_with("--") {
                    d.name.clone()
                } else {
                    d.name.to_ascii_lowercase()
                },
                value: values_text(source, &d.value),
                important: d.important,
            }),
            BlockItem::Rule(_) => None,
        })
        .collect()
}

/// CSSOM "parse a CSS declaration block" for a `style` attribute: the
/// valid declarations Rustic supports, in order, the last of each property
/// winning (custom properties are kept as written).
pub fn parse_declaration_block(source: &str) -> Vec<CssomDeclaration> {
    let parser = CssParser::default();
    let Ok(values) = parser.component_values(source) else {
        return Vec::new();
    };
    let mut out: Vec<CssomDeclaration> = Vec::new();
    for item in parse_block_contents(&values.value, 0).value {
        let BlockItem::Declaration(declaration) = item else {
            continue;
        };
        let custom = declaration.name.starts_with("--");
        if !custom && rustic_css::parse_declaration(&declaration).is_err() {
            continue;
        }
        let parsed = CssomDeclaration {
            name: if custom {
                declaration.name.clone()
            } else {
                declaration.name.to_ascii_lowercase()
            },
            value: values_text(source, &declaration.value),
            important: declaration.important,
        };
        out.retain(|d| d.name != parsed.name);
        out.push(parsed);
    }
    out
}

/// The value text `value` would have as property `name` (CSSOM
/// `setProperty` step 5), or `None` when it does not parse as one value
/// of that property.
pub fn parse_property_value(name: &str, value: &str) -> Option<String> {
    let declarations = parse_declaration_block(&format!("{name}: {value}"));
    match declarations.as_slice() {
        [one] if !one.important && one.name.eq_ignore_ascii_case(name) => Some(one.value.clone()),
        _ => None,
    }
}

/// CSSOM "serialize a CSS declaration block".
pub fn serialize_declaration_block(declarations: &[CssomDeclaration]) -> String {
    let mut out = String::new();
    for d in declarations {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&d.name);
        out.push_str(": ");
        out.push_str(&d.value);
        if d.important {
            out.push_str(" !important");
        }
        out.push(';');
    }
    out
}

fn style_text(prefix: &str, declarations: &[CssomDeclaration], children: &[CssomRule]) -> String {
    let mut out = String::from(prefix);
    out.push_str(" {");
    for d in declarations {
        out.push(' ');
        out.push_str(&d.name);
        out.push_str(": ");
        out.push_str(&d.value);
        if d.important {
            out.push_str(" !important");
        }
        out.push(';');
    }
    for child in children {
        out.push(' ');
        out.push_str(&child.text);
    }
    out.push_str(" }");
    out
}

/// The rules among `items`. In a style rule or `@scope` (`nested`), runs
/// of declarations after the first rule become nested declarations.
fn rules(source: &str, items: Vec<BlockItem>, nested: bool, budget: &mut usize) -> Vec<CssomRule> {
    let mut out = Vec::new();
    let mut pending: Vec<BlockItem> = Vec::new();
    let flush = |pending: &mut Vec<BlockItem>, out: &mut Vec<CssomRule>| {
        if pending.is_empty() {
            return;
        }
        let declarations = declarations(source, pending);
        pending.clear();
        let text = declarations
            .iter()
            .map(|d| {
                format!(
                    "{}: {}{};",
                    d.name,
                    d.value,
                    if d.important { " !important" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        out.push(CssomRule {
            kind: CssomRuleKind::NestedDeclarations,
            text,
            declarations,
            rules: Vec::new(),
        });
    };
    for item in items {
        if *budget == 0 {
            break;
        }
        match item {
            BlockItem::Declaration(_) if nested => pending.push(item),
            BlockItem::Declaration(_) => {}
            BlockItem::Rule(rule) => {
                flush(&mut pending, &mut out);
                *budget -= 1;
                if let Some(rule) = one(source, rule, budget) {
                    out.push(rule);
                }
            }
        }
    }
    flush(&mut pending, &mut out);
    out
}

fn block_items(block: &[Value]) -> Vec<BlockItem> {
    parse_block_contents(block, 0).value
}

/// Split a block into its leading declarations and the rest.
fn split(items: Vec<BlockItem>) -> (Vec<BlockItem>, Vec<BlockItem>) {
    let first_rule = items
        .iter()
        .position(|item| matches!(item, BlockItem::Rule(_)))
        .unwrap_or(items.len());
    let mut items = items;
    let rest = items.split_off(first_rule);
    (items, rest)
}

fn one(source: &str, rule: Rule, budget: &mut usize) -> Option<CssomRule> {
    match rule {
        Rule::Qualified(q) => {
            let selector = values_text(source, &q.prelude);
            let (leading, rest) = split(block_items(&q.block));
            let declarations = declarations(source, &leading);
            let children = rules(source, rest, true, budget);
            Some(CssomRule {
                text: style_text(&selector, &declarations, &children),
                kind: CssomRuleKind::Style { selector },
                declarations,
                rules: children,
            })
        }
        Rule::At(at) => at_rule(source, at, budget),
    }
}

fn at_rule(source: &str, at: AtRule, budget: &mut usize) -> Option<CssomRule> {
    let name = at.name.to_ascii_lowercase();
    let prelude = values_text(source, &at.prelude);
    let whole = text(source, at.span);
    let grouping = |kind: CssomRuleKind, nested: bool, budget: &mut usize| {
        let items = at.block.as_deref().map(block_items).unwrap_or_default();
        let children = rules(source, items, nested, budget);
        let head = if prelude.is_empty() {
            format!("@{name}")
        } else {
            format!("@{name} {prelude}")
        };
        CssomRule {
            text: style_text(&head, &[], &children),
            kind,
            declarations: Vec::new(),
            rules: children,
        }
    };
    Some(match name.as_str() {
        "media" if at.block.is_some() => grouping(
            CssomRuleKind::Media {
                condition: prelude.clone(),
            },
            false,
            budget,
        ),
        "container" if at.block.is_some() => grouping(
            CssomRuleKind::Container {
                condition: prelude.clone(),
            },
            false,
            budget,
        ),
        "supports" if at.block.is_some() => grouping(
            CssomRuleKind::Supports {
                condition: prelude.clone(),
            },
            false,
            budget,
        ),
        "layer" if at.block.is_some() => grouping(
            CssomRuleKind::LayerBlock {
                name: prelude.clone(),
            },
            false,
            budget,
        ),
        "layer" => CssomRule {
            kind: CssomRuleKind::LayerStatement {
                names: prelude
                    .split(',')
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .collect(),
            },
            text: whole,
            declarations: Vec::new(),
            rules: Vec::new(),
        },
        "scope" if at.block.is_some() => {
            let mut start = None;
            let mut end = None;
            let mut after_to = false;
            for value in at.prelude.iter().filter(|v| !v.is_whitespace()) {
                match &value.value {
                    ComponentValue::Block {
                        kind: BlockKind::Paren,
                        contents,
                    } => {
                        let selector = values_text(source, contents);
                        if after_to {
                            end = Some(selector);
                        } else {
                            start = Some(selector);
                        }
                    }
                    _ => after_to = true,
                }
            }
            grouping(CssomRuleKind::Scope { start, end }, true, budget)
        }
        "keyframes" | "-webkit-keyframes" if at.block.is_some() => {
            let items = at.block.as_deref().map(block_items).unwrap_or_default();
            let mut children = Vec::new();
            for item in items {
                if let BlockItem::Rule(Rule::Qualified(q)) = item {
                    let key = values_text(source, &q.prelude);
                    let declarations = declarations(source, &block_items(&q.block));
                    children.push(CssomRule {
                        text: style_text(&key, &declarations, &[]),
                        kind: CssomRuleKind::Keyframe { key },
                        declarations,
                        rules: Vec::new(),
                    });
                }
            }
            let name_text = prelude.trim_matches('"').to_string();
            CssomRule {
                text: style_text(&format!("@{name} {prelude}"), &[], &children),
                kind: CssomRuleKind::Keyframes { name: name_text },
                declarations: Vec::new(),
                rules: children,
            }
        }
        "import" => {
            let mut parts = prelude.splitn(2, ' ');
            let first = parts.next().unwrap_or("");
            let rest = parts.next().unwrap_or("").trim();
            let href = first
                .trim_start_matches("url(")
                .trim_end_matches(')')
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string();
            let mut layer = None;
            let mut supports = None;
            let mut media = rest.to_string();
            if let Some(after) = media.strip_prefix("layer") {
                if let Some(inner) = after.strip_prefix('(') {
                    let close = inner.find(')').unwrap_or(inner.len());
                    layer = Some(inner[..close].to_string());
                    media = inner[(close + 1).min(inner.len())..].trim().to_string();
                } else {
                    layer = Some(String::new());
                    media = after.trim().to_string();
                }
            }
            if let Some(inner) = media.strip_prefix("supports(") {
                let close = inner.rfind(')').unwrap_or(inner.len());
                supports = Some(inner[..close].to_string());
                media = inner[(close + 1).min(inner.len())..].trim().to_string();
            }
            CssomRule {
                kind: CssomRuleKind::Import {
                    href,
                    layer,
                    supports,
                    media,
                },
                text: whole,
                declarations: Vec::new(),
                rules: Vec::new(),
            }
        }
        "namespace" => {
            let mut words = prelude.split_ascii_whitespace();
            let (prefix, uri) = match (words.next(), words.next()) {
                (Some(prefix), Some(uri)) => (prefix.to_string(), uri.to_string()),
                (Some(uri), None) => (String::new(), uri.to_string()),
                _ => (String::new(), String::new()),
            };
            let uri = uri
                .trim_start_matches("url(")
                .trim_end_matches(')')
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string();
            CssomRule {
                kind: CssomRuleKind::Namespace { prefix, uri },
                text: whole,
                declarations: Vec::new(),
                rules: Vec::new(),
            }
        }
        "font-face" | "property" | "page" => {
            let items = at.block.as_deref().map(block_items).unwrap_or_default();
            let declarations = declarations(source, &items);
            let head = if prelude.is_empty() {
                format!("@{name}")
            } else {
                format!("@{name} {prelude}")
            };
            let kind = match name.as_str() {
                "font-face" => CssomRuleKind::FontFace,
                "property" => CssomRuleKind::Property {
                    name: prelude.clone(),
                },
                _ => CssomRuleKind::Page,
            };
            CssomRule {
                text: style_text(&head, &declarations, &[]),
                kind,
                declarations,
                rules: Vec::new(),
            }
        }
        // `@charset` is not a rule in the CSSOM.
        "charset" => return None,
        _ => CssomRule {
            kind: CssomRuleKind::Other,
            text: whole,
            declarations: Vec::new(),
            rules: Vec::new(),
        },
    })
}
