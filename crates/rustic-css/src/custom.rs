//! Custom properties and `var()` substitution (CSS Custom Properties for
//! Cascading Variables Level 1).
//!
//! A custom property (`--name: tokens`) keeps its value as component
//! values. A declaration of an ordinary property whose value contains
//! `var()` is accepted at parse time if its `var()` functions are well
//! formed, kept as an [`UnparsedValue`], and parsed against the property's
//! grammar only after substitution, at computed-value time.
//!
//! Substitution is bounded: a substituted value may hold at most
//! [`MAX_SUBSTITUTED_VALUES`] component values (the budget is shared by all
//! substitutions for one element, which stops exponential expansion such as
//! `--b: var(--a) var(--a); --c: var(--b) var(--b); ...`) and nest at most
//! [`MAX_SUBSTITUTION_DEPTH`] functions and blocks deep. A substitution that
//! would exceed either limit fails, as an invalid reference does.

use std::fmt;
use std::sync::Arc;

use crate::properties::{longhand, longhands_of, shorthand};
use crate::serialize::write_values;
use crate::{ComponentValue, PropertyId, PropertyValue, Token, Value};

/// The most component values (counting those nested in functions and
/// blocks) that substitution may produce for one element.
pub const MAX_SUBSTITUTED_VALUES: usize = 1 << 16;

/// The deepest nesting of functions and blocks a substituted value may have.
pub const MAX_SUBSTITUTION_DEPTH: usize = 128;

/// Whether `name` is a custom property name (a dashed ident).
pub fn is_custom_property_name(name: &str) -> bool {
    name.starts_with("--")
}

/// The value of a custom property: component values, kept as written.
///
/// Values are compared by their component values, including source spans
/// (shared storage compares equal without a walk).
#[derive(Clone, Debug)]
pub struct CustomValue {
    values: Arc<[Value]>,
    /// Component values, counting nested ones.
    size: usize,
    /// Function and block nesting depth (0 for tokens only).
    depth: usize,
    references: bool,
}

/// Facts about a list of component values, gathered without recursion.
struct Shape {
    size: usize,
    depth: usize,
    references: bool,
    /// Every `var()` is well formed and no token is a bad string, a bad URL
    /// or an unmatched closing bracket.
    valid: bool,
}

fn shape(values: &[Value]) -> Shape {
    let mut shape = Shape {
        size: 0,
        depth: 0,
        references: false,
        valid: true,
    };
    let mut stack: Vec<(&[Value], usize)> = vec![(values, 0)];
    while let Some((list, depth)) = stack.pop() {
        shape.depth = shape.depth.max(depth);
        for value in list {
            shape.size += 1;
            match &value.value {
                ComponentValue::Token(
                    Token::BadString
                    | Token::BadUrl
                    | Token::CloseParen
                    | Token::CloseSquare
                    | Token::CloseCurly,
                ) => shape.valid = false,
                ComponentValue::Token(_) => {}
                ComponentValue::Function { name, arguments } => {
                    if name.eq_ignore_ascii_case("var") {
                        shape.references = true;
                        if var_arguments(arguments).is_none() {
                            shape.valid = false;
                        }
                    }
                    stack.push((arguments, depth + 1));
                }
                ComponentValue::Block { contents, .. } => stack.push((contents, depth + 1)),
            }
        }
    }
    shape
}

/// `var()` arguments: the custom property name and the fallback, if any
/// (without surrounding whitespace; possibly empty).
fn var_arguments(arguments: &[Value]) -> Option<(&str, Option<&[Value]>)> {
    let mut rest = arguments;
    while rest.first().is_some_and(Value::is_whitespace) {
        rest = &rest[1..];
    }
    let (first, mut rest) = rest.split_first()?;
    let name = match first.token() {
        Some(Token::Ident(name)) if is_custom_property_name(name) => name.as_str(),
        _ => return None,
    };
    while rest.first().is_some_and(Value::is_whitespace) {
        rest = &rest[1..];
    }
    match rest.split_first() {
        None => Some((name, None)),
        Some((comma, fallback)) if comma.token() == Some(&Token::Comma) => {
            Some((name, Some(trim(fallback))))
        }
        Some(_) => None,
    }
}

fn trim(mut values: &[Value]) -> &[Value] {
    while values.first().is_some_and(Value::is_whitespace) {
        values = &values[1..];
    }
    while values.last().is_some_and(Value::is_whitespace) {
        values = &values[..values.len() - 1];
    }
    values
}

/// Whether `values` contain a `var()` function anywhere.
pub(crate) fn has_references(values: &[Value]) -> bool {
    shape(values).references
}

/// Whether `values` can be kept for substitution: every `var()` is well
/// formed and nothing in them is a bad string, bad URL or unmatched
/// closing bracket.
pub(crate) fn valid_for_substitution(values: &[Value]) -> bool {
    shape(values).valid
}

impl PartialEq for CustomValue {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.values, &other.values)
            || (self.size == other.size && self.values == other.values)
    }
}

impl CustomValue {
    /// A custom property value from component values, or `None` if they
    /// hold a malformed `var()`, a bad string, a bad URL or an unmatched
    /// closing bracket. Leading and trailing whitespace is dropped.
    pub fn new(values: &[Value]) -> Option<Self> {
        let values = trim(values);
        let shape = shape(values);
        shape.valid.then(|| Self {
            values: values.into(),
            size: shape.size,
            depth: shape.depth,
            references: shape.references,
        })
    }

    /// The component values.
    pub fn values(&self) -> &[Value] {
        &self.values
    }

    /// Whether the value contains `var()`.
    pub fn has_references(&self) -> bool {
        self.references
    }

    /// The names of the custom properties the value refers to, including
    /// those in fallbacks, sorted and without repeats.
    pub fn references(&self) -> Vec<&str> {
        let mut names = Vec::new();
        if !self.references {
            return names;
        }
        let mut stack: Vec<&[Value]> = vec![&self.values];
        while let Some(list) = stack.pop() {
            for value in list {
                match &value.value {
                    ComponentValue::Function { name, arguments } => {
                        if name.eq_ignore_ascii_case("var") {
                            if let Some((reference, _)) = var_arguments(arguments) {
                                names.push(reference);
                            }
                        }
                        stack.push(arguments);
                    }
                    ComponentValue::Block { contents, .. } => stack.push(contents),
                    ComponentValue::Token(_) => {}
                }
            }
        }
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Substitute every `var()` in the value, looking custom properties up
    /// with `lookup` (which returns `None` for a guaranteed-invalid value).
    /// Fails when a reference without a fallback is invalid or a limit is
    /// exceeded; `budget` is the number of component values still allowed
    /// (see [`MAX_SUBSTITUTED_VALUES`]).
    pub fn substitute<'a>(
        &self,
        lookup: &dyn Fn(&str) -> Option<&'a CustomValue>,
        budget: &mut usize,
    ) -> Option<CustomValue> {
        if !self.references {
            return Some(self.clone());
        }
        let mut out = Vec::new();
        let mut shape = Built { size: 0, depth: 0 };
        substitute_into(&self.values, 0, lookup, budget, &mut out, &mut shape)?;
        Some(CustomValue {
            values: out.into(),
            size: shape.size,
            depth: shape.depth,
            references: false,
        })
    }
}

impl fmt::Display for CustomValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_values(f, &self.values)
    }
}

/// Size and depth of substituted output.
struct Built {
    size: usize,
    depth: usize,
}

fn charge(budget: &mut usize, amount: usize) -> Option<()> {
    *budget = budget.checked_sub(amount)?;
    Some(())
}

/// Append `values` with `var()` substituted to `out`, at function nesting
/// `depth`. Recursion is bounded by [`MAX_SUBSTITUTION_DEPTH`].
fn substitute_into<'a>(
    values: &[Value],
    depth: usize,
    lookup: &dyn Fn(&str) -> Option<&'a CustomValue>,
    budget: &mut usize,
    out: &mut Vec<Value>,
    built: &mut Built,
) -> Option<()> {
    if depth > MAX_SUBSTITUTION_DEPTH {
        return None;
    }
    built.depth = built.depth.max(depth);
    for value in values {
        match &value.value {
            ComponentValue::Token(_) => {
                charge(budget, 1)?;
                built.size += 1;
                out.push(value.clone());
            }
            ComponentValue::Function { name, arguments } if name.eq_ignore_ascii_case("var") => {
                let (reference, fallback) = var_arguments(arguments)?;
                if let Some(found) = lookup(reference) {
                    if found.references || depth + found.depth > MAX_SUBSTITUTION_DEPTH {
                        return None;
                    }
                    charge(budget, found.size)?;
                    built.size += found.size;
                    built.depth = built.depth.max(depth + found.depth);
                    out.extend(found.values.iter().cloned());
                } else {
                    substitute_into(fallback?, depth, lookup, budget, out, built)?;
                }
            }
            ComponentValue::Function { name, arguments } => {
                charge(budget, 1)?;
                built.size += 1;
                let mut inner = Vec::new();
                substitute_into(arguments, depth + 1, lookup, budget, &mut inner, built)?;
                out.push(Value {
                    value: ComponentValue::Function {
                        name: name.clone(),
                        arguments: inner,
                    },
                    span: value.span,
                });
            }
            ComponentValue::Block { kind, contents } => {
                charge(budget, 1)?;
                built.size += 1;
                let mut inner = Vec::new();
                substitute_into(contents, depth + 1, lookup, budget, &mut inner, built)?;
                out.push(Value {
                    value: ComponentValue::Block {
                        kind: *kind,
                        contents: inner,
                    },
                    span: value.span,
                });
            }
        }
    }
    Some(())
}

/// The value of an ordinary property declaration that contains `var()`:
/// the declared property name (a longhand or shorthand, lowercase) and its
/// component values. Each longhand of a shorthand shares one.
#[derive(Clone, Debug, PartialEq)]
pub struct UnparsedValue {
    property: String,
    value: CustomValue,
    url_base: Option<Arc<str>>,
}

impl UnparsedValue {
    pub(crate) fn new(property: String, value: CustomValue) -> Self {
        Self {
            property,
            value,
            url_base: None,
        }
    }

    /// The serialized base URL of the stylesheet (or document, for a `style`
    /// attribute) that declared the value. `url()` values in the parsed
    /// result resolve against it; `None` leaves them as written.
    pub fn url_base(&self) -> Option<&str> {
        self.url_base.as_deref()
    }

    /// Record the declaring sheet's base URL (see [`Self::url_base`]).
    pub fn set_url_base(&mut self, base: Option<Arc<str>>) {
        self.url_base = base;
    }

    /// The declared property (a shorthand for a longhand set through one).
    pub fn property(&self) -> &str {
        &self.property
    }

    /// The declared component values.
    pub fn value(&self) -> &CustomValue {
        &self.value
    }

    /// Substitute `var()` and parse the result as `longhand`, through the
    /// declared shorthand if there was one. `None` means the declaration is
    /// invalid at computed-value time (the property then acts as `unset`).
    pub fn resolve<'a>(
        &self,
        longhand_id: PropertyId,
        lookup: &dyn Fn(&str) -> Option<&'a CustomValue>,
        budget: &mut usize,
    ) -> Option<PropertyValue> {
        let substituted = self.value.substitute(lookup, budget)?;
        let parts: Vec<&Value> = substituted
            .values()
            .iter()
            .filter(|v| !v.is_whitespace())
            .collect();
        if parts.is_empty() {
            return None;
        }
        let longhands = longhands_of(&self.property)?;
        let position = longhands.iter().position(|&p| p == longhand_id)?;
        if longhands.len() == 1 {
            longhand(longhand_id, &parts)
        } else {
            shorthand(&self.property, &parts)?.into_iter().nth(position)
        }
    }
}

impl fmt::Display for UnparsedValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value)
    }
}
