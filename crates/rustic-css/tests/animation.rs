//! Transition and animation longhands and shorthands (CSS Transitions 1/2,
//! CSS Animations 1/2, Scroll-driven Animations `animation-timeline`).

use rustic_css::{CssParser, PropertyDeclaration, PropertyId, parse_declaration};

fn parse(source: &str) -> Result<Vec<PropertyDeclaration>, rustic_css::PropertyError> {
    let declaration = CssParser::default()
        .declaration(source)
        .unwrap()
        .value
        .unwrap();
    parse_declaration(&declaration)
}

fn value(source: &str) -> String {
    let declarations = parse(source).unwrap_or_else(|e| panic!("{source}: {e}"));
    assert_eq!(declarations.len(), 1, "{source}");
    declarations[0].value.to_string()
}

fn invalid(source: &str) {
    assert!(parse(source).is_err(), "{source} should be invalid");
}

fn longhands(source: &str) -> Vec<(PropertyId, String)> {
    parse(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .into_iter()
        .map(|d| (d.property.longhand().unwrap(), d.value.to_string()))
        .collect()
}

#[test]
fn transition_longhands_parse_and_serialize() {
    assert_eq!(value("transition-property: none"), "none");
    assert_eq!(value("transition-property: ALL"), "all");
    assert_eq!(
        value("transition-property: width, --Foo, Unknown-Prop"),
        "width, --Foo, unknown-prop"
    );
    invalid("transition-property: none, width");
    invalid("transition-property: initial, width");
    invalid("transition-property: width,");
    assert_eq!(value("transition-duration: 500ms, 2s"), "500ms, 2s");
    invalid("transition-duration: -1s");
    invalid("transition-duration: 0");
    assert_eq!(value("transition-delay: -250ms"), "-250ms");
    assert_eq!(value("transition-delay: calc(1s + 500ms)"), "1.5s");
    assert_eq!(value("transition-delay: calc(2 * 1s - 0.5s)"), "1.5s");
    assert_eq!(
        value("transition-behavior: allow-discrete, normal"),
        "allow-discrete, normal"
    );
    invalid("transition-behavior: discrete");
}

#[test]
fn animation_longhands_parse_and_serialize() {
    assert_eq!(
        value("animation-name: none, slide, \"quoted name\""),
        "none, slide, \"quoted name\""
    );
    invalid("animation-name: initial, slide");
    invalid("animation-name: default");
    assert_eq!(value("animation-duration: auto, 1s"), "auto, 1s");
    invalid("animation-duration: -1s");
    assert_eq!(
        value("animation-iteration-count: infinite, 2.5, 0"),
        "infinite, 2.5, 0"
    );
    invalid("animation-iteration-count: -1");
    assert_eq!(
        value("animation-direction: alternate-reverse, normal"),
        "alternate-reverse, normal"
    );
    assert_eq!(value("animation-fill-mode: both, none"), "both, none");
    assert_eq!(value("animation-play-state: paused"), "paused");
    assert_eq!(
        value("animation-composition: add, accumulate"),
        "add, accumulate"
    );
    assert_eq!(
        value(
            "animation-timeline: auto, none, --main, scroll(), scroll(root x), view(inline 10px 20%)"
        ),
        "auto, none, --main, scroll(), scroll(root x), view(inline 10px 20%)"
    );
    assert_eq!(
        value("animation-timeline: scroll(y self)"),
        "scroll(self y)"
    );
    assert_eq!(value("animation-timeline: view(auto)"), "view()");
    invalid("animation-timeline: main");
    invalid("animation-timeline: scroll(x y)");
    invalid("animation-timeline: view(1px 2px 3px)");
}

#[test]
fn transition_shorthand_expands_every_entry() {
    assert_eq!(
        longhands("transition: opacity 1s ease-in 200ms, width 2s allow-discrete"),
        vec![
            (PropertyId::TransitionProperty, "opacity, width".into()),
            (PropertyId::TransitionDuration, "1s, 2s".into()),
            (PropertyId::TransitionTimingFunction, "ease-in, ease".into()),
            (PropertyId::TransitionDelay, "200ms, 0s".into()),
            (
                PropertyId::TransitionBehavior,
                "normal, allow-discrete".into()
            ),
        ]
    );
    assert_eq!(
        longhands("transition: none")[0],
        (PropertyId::TransitionProperty, "none".into())
    );
    assert_eq!(
        longhands("transition: 1s")[0],
        (PropertyId::TransitionProperty, "all".into())
    );
    invalid("transition: none, opacity 1s");
    invalid("transition: 1s 2s 3s");
    invalid("transition: opacity width 1s");
    invalid("transition: -1s opacity");
}

#[test]
fn animation_shorthand_assigns_keywords_before_the_name() {
    let all = longhands("animation: 3s ease-out 1s infinite alternate both paused spin");
    assert_eq!(all[0], (PropertyId::AnimationName, "spin".into()));
    assert_eq!(all[1], (PropertyId::AnimationDuration, "3s".into()));
    assert_eq!(
        all[2],
        (PropertyId::AnimationTimingFunction, "ease-out".into())
    );
    assert_eq!(all[3], (PropertyId::AnimationDelay, "1s".into()));
    assert_eq!(
        all[4],
        (PropertyId::AnimationIterationCount, "infinite".into())
    );
    assert_eq!(all[5], (PropertyId::AnimationDirection, "alternate".into()));
    assert_eq!(all[6], (PropertyId::AnimationFillMode, "both".into()));
    assert_eq!(all[7], (PropertyId::AnimationPlayState, "paused".into()));
    assert_eq!(all[8], (PropertyId::AnimationComposition, "replace".into()));
    assert_eq!(all[9], (PropertyId::AnimationTimeline, "auto".into()));
    // A keyword for another longhand is taken by it first.
    let named = longhands("animation: reverse reverse");
    assert_eq!(named[0], (PropertyId::AnimationName, "reverse".into()));
    assert_eq!(named[5], (PropertyId::AnimationDirection, "reverse".into()));
    let none = longhands("animation: none");
    assert_eq!(none[0], (PropertyId::AnimationName, "none".into()));
    let pair = longhands("animation: a 1s, b 2s 500ms");
    assert_eq!(pair[0], (PropertyId::AnimationName, "a, b".into()));
    assert_eq!(pair[1], (PropertyId::AnimationDuration, "1s, 2s".into()));
    assert_eq!(pair[3], (PropertyId::AnimationDelay, "0s, 500ms".into()));
    invalid("animation: a b");
    invalid("animation: 1s 1s 1s");
    invalid("animation: a,");
}

#[test]
fn css_wide_keywords_and_var_apply_to_every_longhand() {
    let wide = longhands("transition: inherit");
    assert_eq!(wide.len(), 5);
    assert!(wide.iter().all(|(_, v)| v == "inherit"));
    let deferred = parse("animation: var(--a) 1s").unwrap();
    assert_eq!(deferred.len(), 10);
}
