//! Bounded entry points (CSS Syntax §5.3).

use rustic_base::CancellationToken;

use crate::rules::{self, Cursor};
use crate::tokenizer::{self, Diagnostics, SpannedToken};
use crate::values;
use crate::{BlockItem, CssError, Declaration, Diagnostic, DiagnosticKind, Rule, Value};
use crate::{NamespaceMap, SelectorError, SelectorList, parse_selector_list};

/// Independent budgets for one parse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CssLimits {
    /// Maximum UTF-8 input bytes.
    pub max_input_bytes: usize,
    /// Maximum tokens, counted before grouping into component values.
    pub max_tokens: usize,
    /// Maximum simultaneously open functions and blocks.
    pub max_nesting: usize,
    /// Diagnostics kept; later ones are counted in `dropped_diagnostics`.
    pub max_diagnostics: usize,
}

impl Default for CssLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 8 * 1024 * 1024,
            max_tokens: 1_000_000,
            max_nesting: 256,
            max_diagnostics: 1024,
        }
    }
}

/// A parse result with its recoverable diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed<T> {
    /// The parsed value.
    pub value: T,
    /// The first `max_diagnostics` diagnostics in input order of discovery.
    pub diagnostics: Vec<Diagnostic>,
    /// Diagnostics beyond the limit, counted but not kept.
    pub dropped_diagnostics: usize,
}

/// A parsed stylesheet: its top-level rules.
#[derive(Clone, Debug, PartialEq)]
pub struct Stylesheet {
    /// Top-level qualified rules and at-rules in source order.
    pub rules: Vec<Rule>,
}

/// Reusable, stateless CSS parser with explicit limits and cancellation.
///
/// Input is a Rust string (already decoded); byte-stream encoding detection
/// is not implemented. Every result is complete or an error: a fatal
/// [`CssError`] returns nothing partial.
#[derive(Clone, Debug)]
pub struct CssParser {
    limits: CssLimits,
    cancellation: CancellationToken,
}

impl Default for CssParser {
    fn default() -> Self {
        Self::new(CssLimits::default(), CancellationToken::default())
            .expect("valid default CSS limits")
    }
}

impl CssParser {
    /// Create a parser; every limit except `max_diagnostics` must be nonzero.
    pub fn new(limits: CssLimits, cancellation: CancellationToken) -> Result<Self, CssError> {
        if limits.max_input_bytes == 0 || limits.max_tokens == 0 || limits.max_nesting == 0 {
            return Err(CssError::InvalidLimits);
        }
        Ok(Self {
            limits,
            cancellation,
        })
    }

    fn diagnostics(&self) -> Diagnostics {
        Diagnostics {
            limit: self.limits.max_diagnostics,
            ..Diagnostics::default()
        }
    }

    fn finish<T>(value: T, diagnostics: Diagnostics) -> Parsed<T> {
        Parsed {
            value,
            diagnostics: diagnostics.kept,
            dropped_diagnostics: diagnostics.dropped,
        }
    }

    fn tokens(&self, source: &str, d: &mut Diagnostics) -> Result<Vec<SpannedToken>, CssError> {
        if source.len() > self.limits.max_input_bytes {
            return Err(CssError::InputTooLarge);
        }
        self.cancellation.check().map_err(|_| CssError::Cancelled)?;
        tokenizer::tokenize(source, self.limits.max_tokens, &self.cancellation, d)
    }

    fn values(&self, source: &str, d: &mut Diagnostics) -> Result<Vec<Value>, CssError> {
        let tokens = self.tokens(source, d)?;
        values::build(tokens, source.len(), self.limits.max_nesting, d)
    }

    /// Tokenize the input (§4).
    pub fn tokenize(&self, source: &str) -> Result<Parsed<Vec<SpannedToken>>, CssError> {
        let mut d = self.diagnostics();
        let tokens = self.tokens(source, &mut d)?;
        Ok(Self::finish(tokens, d))
    }

    /// Parse a stylesheet (§5.3.3): top-level rules, skipping CDO/CDC.
    pub fn stylesheet(&self, source: &str) -> Result<Parsed<Stylesheet>, CssError> {
        let mut d = self.diagnostics();
        let values = self.values(source, &mut d)?;
        let rules = rules::rule_list(&mut Cursor::new(&values, source.len()), true, &mut d);
        Ok(Self::finish(Stylesheet { rules }, d))
    }

    /// Parse a list of rules (§5.3.4), which keeps CDO/CDC as rule content.
    pub fn rule_list(&self, source: &str) -> Result<Parsed<Vec<Rule>>, CssError> {
        let mut d = self.diagnostics();
        let values = self.values(source, &mut d)?;
        let rules = rules::rule_list(&mut Cursor::new(&values, source.len()), false, &mut d);
        Ok(Self::finish(rules, d))
    }

    /// Parse exactly one rule (§5.3.5); `None` with a diagnostic otherwise.
    pub fn rule(&self, source: &str) -> Result<Parsed<Option<Rule>>, CssError> {
        let mut d = self.diagnostics();
        let values = self.values(source, &mut d)?;
        let mut c = Cursor::new(&values, source.len());
        c.skip_whitespace();
        let rule = match c.token() {
            None if c.at_end() => {
                d.push(DiagnosticKind::EmptyInput, 0, source.len());
                None
            }
            Some(crate::Token::AtKeyword(_)) => Some(Rule::At(rules::at_rule(&mut c, &mut d))),
            _ => rules::qualified_rule(&mut c, false, &mut d).map(Rule::Qualified),
        };
        c.skip_whitespace();
        if rule.is_some() && !c.at_end() {
            d.push(DiagnosticKind::ExtraInput, 0, source.len());
            return Ok(Self::finish(None, d));
        }
        Ok(Self::finish(rule, d))
    }

    /// Parse exactly one declaration (§5.3.6).
    pub fn declaration(&self, source: &str) -> Result<Parsed<Option<Declaration>>, CssError> {
        let mut d = self.diagnostics();
        let values = self.values(source, &mut d)?;
        let first = values.iter().position(|v| !v.is_whitespace());
        let rest = first.map_or(&[][..], |i| &values[i..]);
        let declaration = if rest.is_empty() {
            d.push(DiagnosticKind::EmptyInput, 0, source.len());
            None
        } else {
            rules::declaration(rest, &mut d)
        };
        Ok(Self::finish(declaration, d))
    }

    /// Parse a block's contents (§5.3.8 with CSS Nesting): declarations,
    /// at-rules and nested qualified rules.
    pub fn block_contents(&self, source: &str) -> Result<Parsed<Vec<BlockItem>>, CssError> {
        let mut d = self.diagnostics();
        let values = self.values(source, &mut d)?;
        let items = rules::block_contents(&mut Cursor::new(&values, source.len()), &mut d);
        Ok(Self::finish(items, d))
    }

    /// Parse exactly one component value (§5.3.9), ignoring surrounding whitespace.
    pub fn component_value(&self, source: &str) -> Result<Parsed<Option<Value>>, CssError> {
        let mut d = self.diagnostics();
        let values = self.values(source, &mut d)?;
        let mut nonspace = values.iter().filter(|v| !v.is_whitespace());
        let value = match (nonspace.next(), nonspace.next()) {
            (None, _) => {
                d.push(DiagnosticKind::EmptyInput, 0, source.len());
                None
            }
            (Some(value), None) => Some(value.clone()),
            (Some(_), Some(_)) => {
                d.push(DiagnosticKind::ExtraInput, 0, source.len());
                None
            }
        };
        Ok(Self::finish(value, d))
    }

    /// Parse a selector list, such as `querySelectorAll`'s argument.
    ///
    /// The outer `Result` reports fatal limits; the inner one whether the
    /// input is a valid selector list.
    pub fn selector_list(
        &self,
        source: &str,
        namespaces: &NamespaceMap,
    ) -> Result<Parsed<Result<SelectorList, SelectorError>>, CssError> {
        let mut d = self.diagnostics();
        let values = self.values(source, &mut d)?;
        Ok(Self::finish(parse_selector_list(&values, namespaces), d))
    }

    /// Parse a list of component values (§5.3.10).
    pub fn component_values(&self, source: &str) -> Result<Parsed<Vec<Value>>, CssError> {
        let mut d = self.diagnostics();
        let values = self.values(source, &mut d)?;
        Ok(Self::finish(values, d))
    }
}

/// Read a rule's raw `{}` block as block contents, after the fact.
///
/// Diagnostics refer to the original input through the values' spans.
pub fn parse_block_contents(block: &[Value], max_diagnostics: usize) -> Parsed<Vec<BlockItem>> {
    let mut d = Diagnostics {
        limit: max_diagnostics,
        ..Diagnostics::default()
    };
    let end = block.last().map_or(0, |v| v.span.range().end);
    let items = rules::block_contents(&mut Cursor::new(block, end), &mut d);
    CssParser::finish(items, d)
}
