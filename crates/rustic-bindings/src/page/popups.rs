//! `window.open()` (HTML §7.2.2.1 "window open steps") for a page whose
//! popups open in other processes (ADR 0062, ADR 0057 §3).
//!
//! The page cannot hold a popup: the host loads each one in a renderer of
//! its own after the page's turn, decides its browsing context group from
//! COOP, and never gives this realm a WindowProxy for it, so `open()`
//! returns `null` (TD-178). A popup needs transient user activation, which
//! it consumes (the popup blocker); without it nothing is recorded.
//! `_self`, `_parent` and `_top` would navigate this page, which the CLI
//! host does not do yet, so they open nothing.

use std::cell::RefCell;

use rustic_js::runtime::{Abrupt, Agent, JsResult, PropertyDescriptor, Value};
use rustic_url::Url;

use super::with_page;
use crate::webidl::coerce_string;

/// Most popups a page records before the host takes them.
pub const POPUP_CAPACITY: usize = 16;

/// A popup the page asked for with transient activation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PopupRequest {
    /// The resolved URL (`about:blank` for an empty one).
    pub url: Url,
    /// The `noopener` feature (implied by `noreferrer`).
    pub noopener: bool,
    /// The `noreferrer` feature.
    pub noreferrer: bool,
}

/// The page's recorded popups.
#[derive(Debug, Default)]
pub(super) struct Popups(RefCell<Vec<PopupRequest>>);

impl Popups {
    pub(super) fn take(&self) -> Vec<PopupRequest> {
        std::mem::take(&mut self.0.borrow_mut())
    }
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let function = Value::object(agent.new_native_function(open, "open", 0)?);
    let global = agent.intrinsics.global;
    let key = agent.heap.key("open")?;
    agent.define_property_or_throw(
        global,
        key,
        PropertyDescriptor::data_with(function, true, true, true),
    )
}

/// HTML "tokenize the features" then "check if a window feature is set"
/// for `name` (a boolean feature: present with an empty value, `yes`,
/// `true` or a non-zero number).
fn feature(features: &str, name: &str) -> bool {
    let separator = |c: char| c.is_ascii_whitespace() || c == ',';
    for item in features.split(separator).filter(|item| !item.is_empty()) {
        let (key, value) = item.split_once('=').unwrap_or((item, ""));
        if !key.trim().eq_ignore_ascii_case(name) {
            continue;
        }
        let value = value.trim().to_ascii_lowercase();
        return match value.as_str() {
            "" | "yes" | "true" => true,
            number => number
                .trim_start_matches(['+', '-'])
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|digits| digits.parse::<u64>().ok())
                .is_some_and(|value| value != 0),
        };
    }
    false
}

fn open(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let text = |agent: &mut Agent, index: usize, default: &str| -> JsResult<String> {
        match args.get(index) {
            None => Ok(default.to_owned()),
            Some(&value) if value == Value::UNDEFINED => Ok(default.to_owned()),
            Some(&value) => {
                let string = coerce_string(agent, value)?;
                Ok(char::decode_utf16(agent.heap.string(string).units())
                    .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
                    .collect())
            }
        }
    };
    let input = text(agent, 0, "")?;
    let target = text(agent, 1, "_blank")?;
    let features = text(agent, 2, "")?;
    let noreferrer = feature(&features, "noreferrer");
    let noopener = noreferrer || feature(&features, "noopener");
    let base = with_page(agent, |_, page| {
        Ok(page
            .fetch
            .borrow()
            .as_ref()
            .and_then(|host| host.base_url()))
    })?;
    let url = if input.is_empty() {
        Url::parse("about:blank", None).ok()
    } else {
        Url::parse(&input, base.as_ref()).ok()
    };
    let Some(url) = url else {
        return Err(
            match crate::dom_exception::new(agent, "SyntaxError", "invalid popup URL") {
                Ok(error) => Abrupt::Throw(error),
                Err(error) => error,
            },
        );
    };
    let target = target.to_ascii_lowercase();
    if matches!(target.as_str(), "_self" | "_parent" | "_top") {
        return Ok(Value::NULL);
    }
    // The popup blocker: only a page with transient activation opens one,
    // and opening spends it.
    if !super::activation::consume(agent) {
        return Ok(Value::NULL);
    }
    with_page(agent, |_, page| {
        let mut popups = page.popups.0.borrow_mut();
        if popups.len() < POPUP_CAPACITY {
            popups.push(PopupRequest {
                url,
                noopener,
                noreferrer,
            });
        }
        Ok(Value::NULL)
    })
}

#[cfg(test)]
mod tests {
    use super::feature;

    #[test]
    fn boolean_features_follow_html_tokenizing() {
        assert!(feature("noopener", "noopener"));
        assert!(feature("width=10, NoOpener", "noopener"));
        assert!(feature("noopener=yes", "noopener"));
        assert!(feature("noopener=1", "noopener"));
        assert!(feature("noreferrer=true", "noreferrer"));
        assert!(!feature("noopener=0", "noopener"));
        assert!(!feature("noopener=no", "noopener"));
        assert!(!feature("noopenerx", "noopener"));
        assert!(!feature("", "noopener"));
    }
}
