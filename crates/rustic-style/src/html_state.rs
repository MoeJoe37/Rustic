//! HTML element facts that selectors read: form-control states (HTML
//! Standard §4.16.3), the content language for `:lang()` (§3.2.6.2) and
//! directionality for `:dir()` (§3.2.6.4), all reviewed 2026-11-08.
//!
//! Without user interaction or script, every state comes from content
//! attributes and the tree: checkedness from `checked`, selectedness from
//! `selected` and the select element's selectedness setting algorithm, and
//! values from the `value` attribute or a textarea's text.

use std::collections::HashMap;
use std::rc::Rc;

use rustic_css::FormState;
use rustic_dom::HTML_NAMESPACE;
use rustic_url::idna::{StrongDirection, strong_direction};

use crate::matching::MatchCache;
use crate::{ChildRef, ElementView};

const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
const MATHML_NAMESPACE: &str = "http://www.w3.org/1998/Math/MathML";

/// The element's HTML local name, if it is an HTML element.
fn html_name<E: ElementView>(e: &E) -> Option<&str> {
    (e.namespace() == Some(HTML_NAMESPACE)).then(|| e.local_name())
}

fn is_html<E: ElementView>(e: &E, name: &str) -> bool {
    html_name(e) == Some(name)
}

/// An unnamespaced attribute's value.
fn attr<'a, E: ElementView>(e: &'a E, local: &str) -> Option<&'a [u16]> {
    e.attributes()
        .find(|a| a.namespace.is_none() && a.local == local)
        .map(|a| a.value)
}

fn has_attr<E: ElementView>(e: &E, local: &str) -> bool {
    attr(e, local).is_some()
}

/// ASCII case-insensitive comparison with an ASCII keyword.
fn eq_ci(units: &[u16], keyword: &str) -> bool {
    units.len() == keyword.len()
        && units
            .iter()
            .zip(keyword.bytes())
            .all(|(&u, b)| u8::try_from(u).is_ok_and(|u| u.eq_ignore_ascii_case(&b)))
}

const INPUT_TYPES: &[&str] = &[
    "hidden",
    "text",
    "search",
    "tel",
    "url",
    "email",
    "password",
    "date",
    "month",
    "week",
    "time",
    "datetime-local",
    "number",
    "range",
    "color",
    "checkbox",
    "radio",
    "file",
    "submit",
    "image",
    "reset",
    "button",
];

/// An input element's type state keyword (missing or invalid is `text`).
fn input_type<E: ElementView>(e: &E) -> &'static str {
    attr(e, "type")
        .and_then(|value| INPUT_TYPES.iter().find(|t| eq_ci(value, t)).copied())
        .unwrap_or("text")
}

/// The input type if `e` is an HTML input element.
fn input<E: ElementView>(e: &E) -> Option<&'static str> {
    is_html(e, "input").then(|| input_type(e))
}

fn parent_chain<E: ElementView>(e: E) -> impl Iterator<Item = E> {
    std::iter::successors(e.parent_element(), E::parent_element)
}

/// The tree's root element (the top of `e`'s ancestor chain).
fn tree_root<E: ElementView>(e: E) -> E {
    parent_chain(e).last().unwrap_or(e)
}

/// Elements of the subtree under `root` (excluding `root`) in tree order.
pub(crate) struct Descendants<E> {
    root: E,
    next: Option<E>,
}

impl<E: ElementView> Descendants<E> {
    pub(crate) fn new(root: E) -> Self {
        Self {
            root,
            next: root.first_child_element(),
        }
    }
}

impl<E: ElementView> Iterator for Descendants<E> {
    type Item = E;

    fn next(&mut self) -> Option<E> {
        let current = self.next?;
        self.next = current.first_child_element().or_else(|| {
            let mut node = current;
            loop {
                if node == self.root {
                    return None;
                }
                if let Some(sibling) = node.next_sibling_element() {
                    return Some(sibling);
                }
                node = node.parent_element()?;
            }
        });
        Some(current)
    }
}

/// Whole-tree facts, computed once per match or restyle when a selector
/// first needs them.
pub(crate) struct TreeFacts<E> {
    root: E,
    /// The first element with each nonempty ID.
    ids: HashMap<Vec<u16>, E>,
    /// The checked radio button of each named group: the last one with a
    /// `checked` attribute, as inserting it unchecks the others.
    checked_radios: HashMap<(Option<E>, Vec<u16>), E>,
    /// Each form's default button: its first submit button in tree order.
    default_buttons: HashMap<E, E>,
    /// The pragma-set default language from the last
    /// `<meta http-equiv=content-language>` with a usable value.
    pragma_language: Option<Vec<u16>>,
}

fn is_submit_button<E: ElementView>(e: &E) -> bool {
    match html_name(e) {
        Some("button") => attr(e, "type").is_none_or(|t| !eq_ci(t, "reset") && !eq_ci(t, "button")),
        Some("input") => matches!(input_type(e), "submit" | "image"),
        _ => false,
    }
}

impl<E: ElementView> TreeFacts<E> {
    fn build(root: E) -> Self {
        let elements: Vec<E> = std::iter::once(root)
            .chain(Descendants::new(root))
            .collect();
        let mut ids = HashMap::new();
        for e in &elements {
            if let Some(id) = attr(e, "id").filter(|id| !id.is_empty()) {
                ids.entry(id.to_vec()).or_insert(*e);
            }
        }
        let mut facts = Self {
            root,
            ids,
            checked_radios: HashMap::new(),
            default_buttons: HashMap::new(),
            pragma_language: None,
        };
        for e in &elements {
            if input(e) == Some("radio") && has_attr(e, "checked") {
                if let Some(name) = attr(e, "name").filter(|n| !n.is_empty()) {
                    let key = (facts.form_owner(*e), name.to_vec());
                    facts.checked_radios.insert(key, *e);
                }
            }
            if is_submit_button(e) {
                if let Some(form) = facts.form_owner(*e) {
                    facts.default_buttons.entry(form).or_insert(*e);
                }
            }
            if is_html(e, "meta")
                && attr(e, "http-equiv").is_some_and(|v| eq_ci(v, "content-language"))
            {
                if let Some(content) = attr(e, "content") {
                    if !content.contains(&u16::from(b',')) {
                        let trimmed = trim_ascii_whitespace(content);
                        if !trimmed.is_empty() {
                            facts.pragma_language = Some(trimmed.to_vec());
                        }
                    }
                }
            }
        }
        facts
    }

    /// A form-associated element's form owner: the element its `form`
    /// attribute names if that is a form, otherwise its nearest form
    /// ancestor. The parser's form element pointer is not recorded in the
    /// DOM, so parser association of non-descendants is missed (TD-31).
    fn form_owner(&self, e: E) -> Option<E> {
        match attr(&e, "form") {
            Some(id) => self.ids.get(id).copied().filter(|f| is_html(f, "form")),
            None => parent_chain(e).find(|a| is_html(a, "form")),
        }
    }
}

fn is_html_whitespace(u: u16) -> bool {
    matches!(u, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

fn trim_ascii_whitespace(units: &[u16]) -> &[u16] {
    let start = units.iter().position(|&u| !is_html_whitespace(u));
    let Some(start) = start else {
        return &[];
    };
    let end = units
        .iter()
        .rposition(|&u| !is_html_whitespace(u))
        .unwrap_or(start);
    &units[start..=end]
}

fn facts<E: ElementView>(e: E, cache: &MatchCache<E>) -> Rc<TreeFacts<E>> {
    let root = tree_root(e);
    let mut slot = cache.tree.borrow_mut();
    match &*slot {
        Some(facts) if facts.root == root => Rc::clone(facts),
        _ => {
            let built = Rc::new(TreeFacts::build(root));
            *slot = Some(Rc::clone(&built));
            built
        }
    }
}

/// Button, input, select, textarea, optgroup, option and fieldset elements:
/// the elements `:enabled` and `:disabled` apply to.
fn is_disableable<E: ElementView>(e: &E) -> bool {
    matches!(
        html_name(e),
        Some("button" | "input" | "select" | "textarea" | "optgroup" | "option" | "fieldset")
    )
}

/// Whether `e` is inside a fieldset with a `disabled` attribute, other than
/// inside that fieldset's first legend child.
fn in_disabled_fieldset<E: ElementView>(e: E) -> bool {
    let mut child = e;
    for ancestor in parent_chain(e) {
        if is_html(&ancestor, "fieldset") && has_attr(&ancestor, "disabled") {
            let first_legend =
                std::iter::successors(ancestor.first_child_element(), E::next_sibling_element)
                    .find(|c| is_html(c, "legend"));
            if first_legend != Some(child) {
                return true;
            }
        }
        child = ancestor;
    }
    false
}

/// HTML's "actually disabled" (and "disabled fieldset") for a disableable
/// element.
fn actually_disabled<E: ElementView>(e: E) -> bool {
    match html_name(&e) {
        Some("optgroup") => has_attr(&e, "disabled"),
        Some("option") => {
            has_attr(&e, "disabled")
                || e.parent_element()
                    .is_some_and(|p| is_html(&p, "optgroup") && has_attr(&p, "disabled"))
        }
        _ => has_attr(&e, "disabled") || in_disabled_fieldset(e),
    }
}

/// The select element an option belongs to, if any.
fn option_select<E: ElementView>(option: E) -> Option<E> {
    let parent = option.parent_element()?;
    if is_html(&parent, "select") {
        return Some(parent);
    }
    let grandparent = parent.parent_element()?;
    (is_html(&parent, "optgroup") && is_html(&grandparent, "select")).then_some(grandparent)
}

/// A select's list of options: option children and option children of
/// optgroup children, in tree order.
fn select_options<E: ElementView>(select: E) -> Vec<E> {
    let mut out = Vec::new();
    for child in std::iter::successors(select.first_child_element(), E::next_sibling_element) {
        if is_html(&child, "option") {
            out.push(child);
        } else if is_html(&child, "optgroup") {
            out.extend(
                std::iter::successors(child.first_child_element(), E::next_sibling_element)
                    .filter(|o| is_html(o, "option")),
            );
        }
    }
    out
}

/// A select's display size is 1 without `multiple` and with no `size`
/// above 1; only then does selectedness setting pick a default option.
fn single_line_select<E: ElementView>(select: &E) -> bool {
    if has_attr(select, "multiple") {
        return false;
    }
    attr(select, "size")
        .and_then(parse_non_negative_integer)
        .is_none_or(|size| size <= 1)
}

/// HTML's rules for parsing non-negative integers.
fn parse_non_negative_integer(units: &[u16]) -> Option<u64> {
    let mut rest = units
        .iter()
        .copied()
        .skip_while(|&u| is_html_whitespace(u))
        .peekable();
    if rest.peek() == Some(&u16::from(b'+')) {
        rest.next();
    }
    let mut value: Option<u64> = None;
    for u in rest {
        let Some(digit) = char::from_u32(u32::from(u)).and_then(|c| c.to_digit(10)) else {
            break;
        };
        value = Some(
            value
                .unwrap_or(0)
                .saturating_mul(10)
                .saturating_add(u64::from(digit)),
        );
    }
    value
}

/// An option's selectedness after parsing: its `selected` attribute, except
/// that in a single-line select the last option with `selected` wins, and
/// with none the first non-disabled option is selected.
fn option_selected<E: ElementView>(option: E) -> bool {
    let Some(select) = option_select(option).filter(single_line_select) else {
        return has_attr(&option, "selected");
    };
    let options = select_options(select);
    let chosen = options
        .iter()
        .rev()
        .find(|o| has_attr(*o, "selected"))
        .or_else(|| options.iter().find(|o| !actually_disabled(**o)));
    chosen == Some(&option)
}

fn checked<E: ElementView>(e: E, cache: &MatchCache<E>) -> bool {
    match html_name(&e) {
        Some("input") => match input_type(&e) {
            "checkbox" => has_attr(&e, "checked"),
            "radio" => {
                if !has_attr(&e, "checked") {
                    return false;
                }
                match attr(&e, "name").filter(|n| !n.is_empty()) {
                    None => true,
                    Some(name) => {
                        let facts = facts(e, cache);
                        let key = (facts.form_owner(e), name.to_vec());
                        facts.checked_radios.get(&key) == Some(&e)
                    }
                }
            }
            _ => false,
        },
        Some("option") => option_selected(e),
        _ => false,
    }
}

fn indeterminate<E: ElementView>(e: E, cache: &MatchCache<E>) -> bool {
    match html_name(&e) {
        // A checkbox is indeterminate only through its IDL attribute.
        Some("input") if input_type(&e) == "radio" => {
            match attr(&e, "name").filter(|n| !n.is_empty()) {
                None => !has_attr(&e, "checked"),
                Some(name) => {
                    let facts = facts(e, cache);
                    let key = (facts.form_owner(e), name.to_vec());
                    !facts.checked_radios.contains_key(&key)
                }
            }
        }
        Some("progress") => !has_attr(&e, "value"),
        _ => false,
    }
}

fn is_default<E: ElementView>(e: E, cache: &MatchCache<E>) -> bool {
    if is_submit_button(&e) {
        let facts = facts(e, cache);
        return facts
            .form_owner(e)
            .is_some_and(|form| facts.default_buttons.get(&form) == Some(&e));
    }
    match html_name(&e) {
        Some("input") => matches!(input_type(&e), "checkbox" | "radio") && has_attr(&e, "checked"),
        Some("option") => has_attr(&e, "selected"),
        _ => false,
    }
}

/// Input types the `required` attribute applies to.
fn required_applies(input_type: &str) -> bool {
    !matches!(
        input_type,
        "hidden" | "range" | "color" | "submit" | "image" | "reset" | "button"
    )
}

/// Input types the `readonly` attribute applies to.
fn readonly_applies(input_type: &str) -> bool {
    matches!(
        input_type,
        "text"
            | "search"
            | "url"
            | "tel"
            | "email"
            | "password"
            | "date"
            | "month"
            | "week"
            | "time"
            | "datetime-local"
            | "number"
    )
}

/// `None` when `required` does not apply, else whether it is set.
fn required<E: ElementView>(e: &E) -> Option<bool> {
    match html_name(e) {
        Some("input") => required_applies(input_type(e)).then(|| has_attr(e, "required")),
        Some("select" | "textarea") => Some(has_attr(e, "required")),
        _ => None,
    }
}

/// Whether `e` is an editing host or editable through `contenteditable`
/// (HTML §6.8.1); the nearest valid state on the ancestor chain decides.
fn editable<E: ElementView>(e: E) -> bool {
    for node in std::iter::once(e).chain(parent_chain(e)) {
        if html_name(&node).is_none() {
            continue;
        }
        if let Some(value) = attr(&node, "contenteditable") {
            if value.is_empty() || eq_ci(value, "true") || eq_ci(value, "plaintext-only") {
                return true;
            }
            if eq_ci(value, "false") {
                return false;
            }
        }
    }
    false
}

fn read_write<E: ElementView>(e: E) -> bool {
    match html_name(&e) {
        Some("input") => {
            readonly_applies(input_type(&e)) && !has_attr(&e, "readonly") && !actually_disabled(e)
        }
        Some("textarea") => !has_attr(&e, "readonly") && !actually_disabled(e),
        _ => editable(e),
    }
}

/// HTML's "valid floating-point number".
fn valid_float(units: &[u16]) -> bool {
    let s: Vec<u8> = match units.iter().map(|&u| u8::try_from(u).ok()).collect() {
        Some(s) => s,
        None => return false,
    };
    let mut i = usize::from(s.first() == Some(&b'-'));
    let digits = |i: &mut usize| {
        let start = *i;
        while s.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i > start
    };
    let integer = digits(&mut i);
    let fraction = if s.get(i) == Some(&b'.') {
        i += 1;
        if !digits(&mut i) {
            return false;
        }
        true
    } else {
        false
    };
    if !integer && !fraction {
        return false;
    }
    if matches!(s.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(s.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == s.len()
}

/// Whether an input's value, after its type's value sanitization, is empty.
fn input_value_empty<E: ElementView>(e: &E, input_type: &str) -> bool {
    let value = attr(e, "value").unwrap_or(&[]);
    let without_newlines = || value.iter().all(|&u| u == 0x0A || u == 0x0D);
    match input_type {
        "url" | "email" => value
            .iter()
            .all(|&u| u == 0x0A || u == 0x0D || is_html_whitespace(u)),
        "number" => !valid_float(value),
        _ => without_newlines(),
    }
}

fn placeholder_shown<E: ElementView>(e: &E) -> bool {
    if !has_attr(e, "placeholder") {
        return false;
    }
    match html_name(e) {
        Some("input") => {
            let t = input_type(e);
            matches!(
                t,
                "text" | "search" | "url" | "tel" | "email" | "password" | "number"
            ) && input_value_empty(e, t)
        }
        Some("textarea") => e.children().all(|child| match child {
            ChildRef::Text(data) => data.is_empty(),
            ChildRef::Element(_) => true,
        }),
        _ => false,
    }
}

/// Whether `e` is in a form-control state.
pub(crate) fn form_state<E: ElementView>(state: FormState, e: E, cache: &MatchCache<E>) -> bool {
    match state {
        FormState::Enabled => is_disableable(&e) && !actually_disabled(e),
        FormState::Disabled => is_disableable(&e) && actually_disabled(e),
        FormState::Checked => checked(e, cache),
        FormState::Indeterminate => indeterminate(e, cache),
        FormState::Default => is_default(e, cache),
        FormState::Required => required(&e) == Some(true),
        FormState::Optional => required(&e) == Some(false),
        FormState::ReadWrite => read_write(e),
        FormState::ReadOnly => !read_write(e),
        FormState::PlaceholderShown => placeholder_shown(&e),
    }
}

/// The element's content language: the nearest `xml:lang`, or `lang` on an
/// HTML, SVG or MathML element, then the pragma-set default language. An
/// empty value means the language is unknown.
fn content_language<E: ElementView>(e: E, cache: &MatchCache<E>) -> Option<Vec<u16>> {
    for node in std::iter::once(e).chain(parent_chain(e)) {
        if let Some(lang) = node
            .attributes()
            .find(|a| a.namespace == Some(XML_NAMESPACE) && a.local == "lang")
        {
            return Some(lang.value.to_vec());
        }
        if matches!(
            node.namespace(),
            Some(HTML_NAMESPACE | SVG_NAMESPACE | MATHML_NAMESPACE)
        ) {
            if let Some(lang) = attr(&node, "lang") {
                return Some(lang.to_vec());
            }
        }
    }
    facts(e, cache).pragma_language.clone()
}

/// RFC 4647 §3.3.2 extended filtering, ASCII case-insensitively.
fn extended_filter(range: &str, tag: &str) -> bool {
    let mut range = range.split('-');
    let mut tag = tag.split('-');
    let (Some(first_range), Some(first_tag)) = (range.next(), tag.next()) else {
        return false;
    };
    if first_range != "*" && !first_range.eq_ignore_ascii_case(first_tag) {
        return false;
    }
    let mut tag = tag.peekable();
    for subtag in range {
        if subtag == "*" {
            continue;
        }
        loop {
            let Some(candidate) = tag.next() else {
                return false;
            };
            if candidate.eq_ignore_ascii_case(subtag) {
                break;
            }
            if candidate.len() == 1 {
                return false;
            }
        }
    }
    true
}

/// `:lang()`: whether the content language matches any range. A nonempty
/// range uses extended filtering against a known language; the empty range
/// matches only an explicitly unknown (empty) language.
pub(crate) fn lang_matches<E: ElementView>(ranges: &[String], e: E, cache: &MatchCache<E>) -> bool {
    let Some(language) = content_language(e, cache) else {
        return false;
    };
    let language = String::from_utf16_lossy(&language);
    ranges.iter().any(|range| {
        if range.is_empty() || language.is_empty() {
            range.is_empty() && language.is_empty()
        } else {
            extended_filter(range, &language)
        }
    })
}

/// How an element's own markup decides its directionality.
enum OwnDirection {
    Fixed(StrongDirection),
    /// `dir=auto` (or a `bdi` without a valid `dir`): the first strong
    /// character, inheriting when there is none.
    Auto(Option<StrongDirection>),
    Inherit,
}

fn first_strong(units: &[u16]) -> Option<StrongDirection> {
    char::decode_utf16(units.iter().copied())
        .filter_map(Result::ok)
        .find_map(strong_direction)
}

fn valid_dir<E: ElementView>(e: &E) -> Option<&'static str> {
    let value = attr(e, "dir")?;
    ["ltr", "rtl", "auto"]
        .into_iter()
        .find(|keyword| eq_ci(value, keyword))
}

/// The first strong character of the text in `e`, skipping `bdi`, `script`,
/// `style` and `textarea` elements and elements with a valid `dir`
/// ("contained text auto directionality").
fn contained_text_direction<E: ElementView>(e: E) -> Option<StrongDirection> {
    enum Item<E> {
        Strong(Option<StrongDirection>),
        Element(E),
    }
    let items = |node: E| -> Vec<Item<E>> {
        node.children()
            .map(|child| match child {
                ChildRef::Text(data) => Item::Strong(first_strong(data)),
                ChildRef::Element(element) => Item::Element(element),
            })
            .collect()
    };
    let mut stack = vec![(items(e), 0usize)];
    while let Some((list, index)) = stack.last_mut() {
        let Some(item) = list.get(*index) else {
            stack.pop();
            continue;
        };
        *index += 1;
        match item {
            Item::Strong(Some(direction)) => return Some(*direction),
            Item::Strong(None) => {}
            Item::Element(child) => {
                let child = *child;
                let skipped = matches!(
                    html_name(&child),
                    Some("bdi" | "script" | "style" | "textarea")
                ) || (html_name(&child).is_some() && valid_dir(&child).is_some());
                if !skipped {
                    stack.push((items(child), 0));
                }
            }
        }
    }
    None
}

fn own_direction<E: ElementView>(e: E) -> OwnDirection {
    let Some(name) = html_name(&e) else {
        return OwnDirection::Inherit;
    };
    let auto = || {
        let text_input = name == "input"
            && matches!(input_type(&e), "text" | "search" | "tel" | "url" | "email");
        if text_input {
            OwnDirection::Auto(first_strong(attr(&e, "value").unwrap_or(&[])))
        } else if name == "textarea" {
            OwnDirection::Auto(e.children().find_map(|child| match child {
                ChildRef::Text(data) => first_strong(data),
                ChildRef::Element(_) => None,
            }))
        } else {
            OwnDirection::Auto(contained_text_direction(e))
        }
    };
    match valid_dir(&e) {
        Some("ltr") => OwnDirection::Fixed(StrongDirection::Ltr),
        Some("rtl") => OwnDirection::Fixed(StrongDirection::Rtl),
        Some(_) => auto(),
        None if name == "bdi" => auto(),
        None if name == "input" && input_type(&e) == "tel" => {
            OwnDirection::Fixed(StrongDirection::Ltr)
        }
        None => OwnDirection::Inherit,
    }
}

/// The element's directionality; the root's default is `ltr`. Results are
/// memoized for the whole match or restyle.
pub(crate) fn directionality<E: ElementView>(e: E, cache: &MatchCache<E>) -> StrongDirection {
    // Walk up until an element decides on its own or is already known.
    let mut pending = Vec::new();
    let mut node = Some(e);
    let mut resolved = StrongDirection::Ltr;
    while let Some(current) = node {
        if let Some(&known) = cache.directions.borrow().get(&current) {
            resolved = known;
            break;
        }
        match own_direction(current) {
            OwnDirection::Fixed(direction) | OwnDirection::Auto(Some(direction)) => {
                cache.directions.borrow_mut().insert(current, direction);
                resolved = direction;
                break;
            }
            OwnDirection::Auto(None) | OwnDirection::Inherit => pending.push(current),
        }
        node = current.parent_element();
    }
    let mut directions = cache.directions.borrow_mut();
    for element in pending {
        directions.insert(element, resolved);
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_filtering_follows_rfc_4647() {
        assert!(extended_filter("de", "de-DE"));
        assert!(extended_filter("de-DE", "de-Latn-DE"));
        assert!(extended_filter("de-DE", "de-DE-x-goethe"));
        assert!(extended_filter("*-DE", "de-Latn-DE"));
        assert!(extended_filter("de-*-DE", "DE-latn-de"));
        assert!(!extended_filter("de-DE", "de-x-DE"));
        assert!(!extended_filter("de-DE", "de"));
        assert!(!extended_filter("en", "de"));
        assert!(extended_filter("*", "fr"));
    }

    #[test]
    fn floating_point_numbers_follow_html() {
        for valid in ["0", "-1", "1.5", ".5", "1e3", "1E-3", "-0.25e+2"] {
            assert!(
                valid_float(&valid.encode_utf16().collect::<Vec<_>>()),
                "{valid}"
            );
        }
        for invalid in ["", "-", "1.", "+1", "1e", "e3", " 1", "1,5", "Infinity"] {
            assert!(
                !valid_float(&invalid.encode_utf16().collect::<Vec<_>>()),
                "{invalid}"
            );
        }
    }

    #[test]
    fn non_negative_integers_parse_like_html() {
        let parse = |s: &str| parse_non_negative_integer(&s.encode_utf16().collect::<Vec<_>>());
        assert_eq!(parse(" +4x"), Some(4));
        assert_eq!(parse("1"), Some(1));
        assert_eq!(parse("-1"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("99999999999999999999999"), Some(u64::MAX));
    }
}
