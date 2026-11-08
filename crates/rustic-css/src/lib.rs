//! Bounded CSS syntax tokenization and parsing for Rustic.
//!
//! Implements CSS Syntax Module Level 3 (W3C Editor's Draft, reviewed
//! 2026-11-08): tokenization with escapes, comments and bad-string/bad-URL
//! recovery; component values; and the stylesheet, rule-list, rule,
//! declaration, block-contents (including CSS Nesting) and component-value
//! entry points. Every node carries a byte span into the caller's input.
//!
//! Fatal limits (input bytes, tokens, nesting, cancellation) return a
//! [`CssError`] with no partial result; recoverable syntax errors are kept as
//! bounded [`Diagnostic`]s while parsing continues. All algorithms are
//! iterative.
//!
//! Selector syntax and specificity (Selectors Level 4, the CSS-02 subset)
//! and the `An+B` microsyntax are parsed from component values here, as are
//! typed values for the CSS-03 property set (colors, lengths, box model,
//! display and font basics). Selector matching and the cascade live elsewhere.

#![warn(missing_docs)]

mod anb;
mod color;
mod error;
mod length;
mod media;
mod parser;
mod properties;
mod rules;
mod selector;
mod serialize;
mod token;
mod tokenizer;
mod values;

pub use anb::parse_an_plus_b;
pub use color::{Color, LabSpace, PredefinedSpace};
pub use error::{CssError, Diagnostic, DiagnosticKind};
pub use length::{Length, LengthPercentage, LengthUnit};
pub use media::{
    ColorScheme, MAX_MEDIA_NESTING, MediaComparison, MediaCondition, MediaEnvironment,
    MediaFeature, MediaFeatureName, MediaFeatureValue, MediaPrefix, MediaQualifier, MediaQuery,
    MediaQueryList, MediaType, PointerAccuracy, ResolutionUnit, parse_media_query_list,
};
pub use parser::{CssLimits, CssParser, Parsed, Stylesheet, parse_block_contents};
pub use properties::{
    BorderStyle, BorderWidth, BoxSizing, CssWideKeyword, DeclaredValue, Display, FamilyName,
    FontSize, FontSizeKeyword, FontStyle, FontWeight, GenericFamily, LengthPercentageOrAuto,
    LineHeight, MaxSize, PropertyDeclaration, PropertyError, PropertyId, PropertyValue,
    parse_declaration,
};
pub use rules::{AtRule, BlockItem, Declaration, QualifiedRule, Rule};
pub use selector::{
    AttributeCase, AttributeOperator, AttributeSelector, AttributeValue, Combinator,
    ComplexSelector, Compound, ElementState, MAX_SELECTOR_NESTING, NamespaceMap, NamespaceSelector,
    Nth, NthKind, PseudoClass, PseudoElement, SelectorError, SelectorErrorKind, SelectorList,
    SimpleSelector, Specificity, parse_selector_list,
};
pub use serialize::{serialize_identifier, serialize_string};
pub use token::{Numeric, Token};
pub use tokenizer::SpannedToken;
pub use values::{BlockKind, ComponentValue, Value};
