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
//! display and font basics) and the math functions `calc()`, `min()`, `max()`
//! and `clamp()` ([`Calc`]), custom properties and bounded `var()`
//! substitution ([`CustomValue`], [`UnparsedValue`]). Selector matching and
//! the cascade live elsewhere.

#![warn(missing_docs)]

mod anb;
mod background;
mod calc;
mod color;
mod container;
mod custom;
mod easing;
mod effects;
mod error;
mod flex;
mod length;
mod media;
mod parser;
mod properties;
mod registered;
mod rules;
mod selector;
mod serialize;
mod shape;
mod token;
mod tokenizer;
mod transform;
mod values;

pub use anb::parse_an_plus_b;
pub use background::{
    Attachment, BackgroundRepeat, BackgroundSize, Image, PositionComponent, PositionSide,
    PositionValue, RepeatStyle, VisualBox,
};
pub use calc::{Calc, CalcKind, CalcNode, MAX_CALC_DEPTH, MAX_CALC_NODES};
pub use color::{Color, LabSpace, PredefinedSpace};
pub use container::{
    ContainerAxes, ContainerCondition, ContainerFacts, ContainerQueryList, StyleQuery,
    parse_container_query_list,
};
pub use custom::{
    CustomValue, MAX_SUBSTITUTED_VALUES, MAX_SUBSTITUTION_DEPTH, UnparsedValue,
    is_custom_property_name,
};
pub use easing::{EasingFunction, EasingList};
pub use effects::{BlendMode, FilterFunction, Isolation, Shadow};
pub use error::{CssError, Diagnostic, DiagnosticKind};
pub use flex::{
    AlignKeyword, Alignment, FlexBasis, FlexDirection, FlexWrap, Gap, OverflowPosition, Size,
};
pub use length::{Length, LengthPercentage, LengthUnit};
pub use media::{
    ColorScheme, FontUnitRatios, MAX_MEDIA_NESTING, MediaComparison, MediaCondition,
    MediaEnvironment, MediaFeature, MediaFeatureName, MediaFeatureValue, MediaPrefix,
    MediaQualifier, MediaQuery, MediaQueryList, MediaType, PointerAccuracy, ResolutionUnit,
    parse_media_query_list,
};
pub use parser::{CssLimits, CssParser, Parsed, Stylesheet, parse_block_contents};
pub use properties::{
    BorderStyle, BorderWidth, BoxSizing, Clear, ContainerType, CornerRadius, CssWideKeyword,
    DeclaredValue, Direction, Display, FamilyName, Float, FontSize, FontSizeKeyword, FontStretch,
    FontStretchKeyword, FontStyle, FontVariantCaps, FontWeight, GenericFamily, Hyphens,
    LengthPercentageOrAuto, LineBreak, LineHeight, MaxSize, OutlineStyle, Overflow, OverflowWrap,
    Position, PropertyDeclaration, PropertyError, PropertyId, PropertyName, PropertyValue, Spacing,
    TextAlign, TextCase, TextDecorationLine, TextDecorationStyle, TextTransform, UnicodeBidi,
    VerticalAlign, VerticalAlignKeyword, Visibility, WhiteSpace, WordBreak, ZIndex,
    parse_declaration,
};
pub use registered::{
    PropertySyntax, RegisteredItem, RegisteredValue, SyntaxComponent, SyntaxMultiplier, SyntaxType,
};
pub use rules::{AtRule, BlockItem, Declaration, QualifiedRule, Rule};
pub use selector::{
    AttributeCase, AttributeOperator, AttributeSelector, AttributeValue, Combinator,
    ComplexSelector, Compound, ElementState, FormState, MAX_SELECTOR_NESTING, NamespaceMap,
    NamespaceSelector, Nth, NthKind, PseudoClass, PseudoElement, RelativeSelector,
    RelativeSelectorList, SelectorError, SelectorErrorKind, SelectorList, SimpleSelector,
    Specificity, parse_nested_selector_list, parse_scoped_selector_list, parse_selector_list,
};
pub use serialize::{serialize_identifier, serialize_string};
pub use shape::{BasicShape, ClipPath, GeometryBox, ShapeRadius};
pub use token::{Numeric, Token};
pub use tokenizer::SpannedToken;
pub use transform::{
    BackfaceVisibility, IndividualTransform, TransformFunction, TransformList, TransformStyle,
};
pub use values::{BlockKind, ComponentValue, Value};
