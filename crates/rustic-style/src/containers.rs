//! Container query evaluation for the cascade (CSS Conditional Rules
//! Level 5 §6).
//!
//! An `@container` condition picks the nearest ancestor that is a query
//! container for it: one whose `container-name` lists the condition's name
//! (if any) and whose `container-type` gives sizes on every axis the query
//! tests (`inline-size` for the inline axis, `size` for both). Every element
//! is a style container, so a condition with only `style()` tests picks the
//! nearest ancestor with the name, or the parent. Without such an ancestor
//! the condition is false.
//!
//! Sizes come from layout, which Rustic does not have yet: callers supply
//! them per element ([`ContainerSize`]); a size feature on an element
//! without a supplied size is unknown, so the rule does not apply.

use rustic_css::{
    ContainerCondition, ContainerFacts, ContainerQueryList, ContainerType, CustomValue,
    FontUnitRatios, MAX_SUBSTITUTED_VALUES, MediaEnvironment, PropertyId, PropertyValue,
};

use crate::ComputedStyle;
use crate::fonts::unit_ratios;

/// A query container's content-box size in CSS pixels, from layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContainerSize {
    /// Width (the inline size; writing modes are not supported yet).
    pub width: f64,
    /// Height (the block size).
    pub height: f64,
}

/// An ancestor that container queries can consult: its computed style and,
/// if layout has provided one, its size.
#[derive(Clone, Copy, Debug)]
pub struct ContainerInfo<'a, E> {
    /// The ancestor element.
    pub element: E,
    /// Its computed style.
    pub style: &'a ComputedStyle,
    /// Its content-box size, if known.
    pub size: Option<ContainerSize>,
}

fn container_type(style: &ComputedStyle) -> ContainerType {
    match style.get(PropertyId::ContainerType) {
        PropertyValue::ContainerType(t) => *t,
        _ => ContainerType::Normal,
    }
}

fn has_name(style: &ComputedStyle, name: &str) -> bool {
    match style.get(PropertyId::ContainerName) {
        PropertyValue::ContainerName(names) => names.iter().any(|n| n == name),
        _ => false,
    }
}

/// Whether a `style()` feature holds on `style`: custom properties only;
/// any other property is unknown.
fn style_feature(style: &ComputedStyle, name: &str, value: Option<&CustomValue>) -> Option<bool> {
    if !rustic_css::is_custom_property_name(name) {
        return None;
    }
    let custom = style.custom_properties();
    let actual = custom.get(name);
    let Some(value) = value else {
        return Some(actual.is_some());
    };
    let mut budget = MAX_SUBSTITUTED_VALUES;
    let wanted = value.substitute(&|n| custom.get(n), &mut budget);
    Some(match (wanted, actual) {
        (Some(wanted), Some(actual)) => wanted.to_string() == actual.to_string(),
        _ => false,
    })
}

fn condition_matches<E>(
    condition: &ContainerCondition,
    containers: &[ContainerInfo<'_, E>],
    environment: &MediaEnvironment,
) -> bool {
    let axes = condition.axes();
    let found = containers.iter().rev().find(|info| {
        let kind = container_type(info.style);
        let inline_ok = matches!(kind, ContainerType::Size | ContainerType::InlineSize);
        let block_ok = kind == ContainerType::Size;
        condition
            .name
            .as_deref()
            .is_none_or(|n| has_name(info.style, n))
            && (!axes.inline || inline_ok)
            && (!axes.block || block_ok)
    });
    let Some(container) = found else {
        return false;
    };
    let Some(query) = &condition.query else {
        return true;
    };
    let kind = container_type(container.style);
    let (root_font, root_ratios) =
        containers
            .first()
            .map_or((16.0, FontUnitRatios::FALLBACK), |root| {
                (
                    root.style.font_size_px(),
                    unit_ratios(root.style.font_metrics()),
                )
            });
    let facts = ContainerFacts {
        width: container
            .size
            .filter(|_| matches!(kind, ContainerType::Size | ContainerType::InlineSize))
            .map(|s| s.width),
        height: container
            .size
            .filter(|_| kind == ContainerType::Size)
            .map(|s| s.height),
        font_size_px: container.style.font_size_px(),
        root_font_size_px: root_font,
        font_ratios: unit_ratios(container.style.font_metrics()),
        root_font_ratios: root_ratios,
        viewport_width: environment.width,
        viewport_height: environment.height,
    };
    let style =
        |name: &str, value: Option<&CustomValue>| style_feature(container.style, name, value);
    query.evaluate_container(&facts, &style) == Some(true)
}

/// Whether any condition of `list` holds for an element whose ancestors
/// are `containers` (root first).
pub(crate) fn container_list_matches<E>(
    list: &ContainerQueryList,
    containers: &[ContainerInfo<'_, E>],
    environment: &MediaEnvironment,
) -> bool {
    list.conditions
        .iter()
        .any(|c| condition_matches(c, containers, environment))
}
