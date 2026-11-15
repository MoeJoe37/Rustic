use rustic_css::{CssParser, EasingFunction};

fn parse(source: &str) -> Option<EasingFunction> {
    let value = CssParser::default()
        .component_value(source)
        .unwrap()
        .value?;
    EasingFunction::parse(&value)
}

fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-10, "{actual} != {expected}");
}

#[test]
fn linear_easing_normalizes_stops_and_samples_discontinuities() {
    let curve = parse("linear(0, 0.25 50% 75%, 0.5 60%, 1)").unwrap();
    assert_eq!(
        curve.to_string(),
        "linear(0 0%, 0.25 50%, 0.25 75%, 0.5 75%, 1 100%)"
    );
    near(curve.sample(-0.5, false), -0.25);
    near(curve.sample(0.625, false), 0.25);
    near(curve.sample(0.75, false), 0.5);
    near(curve.sample(1.25, false), 1.5);

    let implicit = parse("linear(0, 0.1, 0.2, 1 60%)").unwrap();
    assert_eq!(
        implicit.to_string(),
        "linear(0 0%, 0.1 20%, 0.2 40%, 1 60%)"
    );
    near(implicit.sample(0.3, false), 0.15);
    let math = parse("linear(0, calc(20% + 50%) calc(1 / 2), 1)").unwrap();
    near(math.sample(0.7, false), 0.5);
    let flat = parse("linear(1, 1)").unwrap();
    assert_eq!(flat.sample(f64::NEG_INFINITY, false), 1.0);
}

#[test]
fn cubic_bezier_inverts_x_and_extends_endpoint_tangents() {
    let ease = parse("EASE").unwrap();
    near(ease.sample(0.5, false), 0.802_403_387_584_857);
    assert_eq!(ease.to_string(), "ease");
    let curve = parse("cubic-bezier(0, -0.2, 0.5, 2)").unwrap();
    near(curve.sample(-0.5, false), -2.0);
    near(curve.sample(1.5, false), 0.0);
    near(curve.sample(0.0, false), 0.0);
    near(curve.sample(1.0, false), 1.0);
    let horizontal = parse("cubic-bezier(0.25, 0, 0.75, 1)").unwrap();
    assert_eq!(horizontal.sample(f64::NEG_INFINITY, false), 0.0);
    assert_eq!(horizontal.sample(f64::INFINITY, false), 1.0);
}

#[test]
fn step_easing_handles_the_before_flag_and_endpoint_clamping() {
    let start = parse("step-start").unwrap();
    assert_eq!(start.to_string(), "steps(1, start)");
    assert_eq!(start.sample(0.0, true), 0.0);
    assert_eq!(start.sample(0.0, false), 1.0);
    assert_eq!(start.sample(1.0, false), 1.0);
    let end = parse("steps(4, jump-end)").unwrap();
    assert_eq!(end.to_string(), "steps(4)");
    assert_eq!(end.sample(0.75, true), 0.5);
    assert_eq!(end.sample(0.75, false), 0.75);
    assert_eq!(end.sample(-0.125, false), -0.25);
    let none = parse("steps(2, jump-none)").unwrap();
    assert_eq!(none.sample(0.5, true), 0.0);
    assert_eq!(none.sample(0.5, false), 1.0);
    let both = parse("steps(3, jump-both)").unwrap();
    assert_eq!(both.sample(0.0, false), 0.25);
    assert_eq!(both.sample(1.0, true), 0.75);
}

#[test]
fn invalid_easing_grammar_is_rejected() {
    for source in [
        "linear(0)",
        "linear(0 10% 20% 30%, 1)",
        "linear(10% 0 20%, 1)",
        "linear(0 10px, 1)",
        "linear(0, 1,)",
        "steps(0)",
        "steps(1, jump-none)",
        "steps(2.0, start)",
        "steps(2 start)",
        "steps(2, middle)",
        "cubic-bezier(1.01, 0, 0.5, 1)",
        "cubic-bezier(0 0, 0.5, 1)",
        "cubic-bezier(0.5, 0, -0.1, 1)",
    ] {
        assert!(parse(source).is_none(), "{source}");
    }
}
