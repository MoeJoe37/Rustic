//! Math functions: `calc()`, `min()`, `max()` and `clamp()` (CSS Values 4 §10).

use rustic_css::{Calc, CssParser, Length, LengthUnit, MAX_CALC_DEPTH, MAX_CALC_NODES, Value};

fn value(source: &str) -> Value {
    CssParser::default()
        .component_value(source)
        .unwrap()
        .value
        .unwrap_or_else(|| panic!("{source:?} is not one component value"))
}

fn lp(source: &str) -> Option<String> {
    Calc::parse_length_percentage(&value(source)).map(|c| c.to_string())
}

#[test]
fn length_percentage_calculations_simplify_and_serialize() {
    let cases = [
        ("calc(1px + 2px)", "calc(3px)"),
        ("CALC(1PX + 1Px)", "calc(2px)"),
        ("calc(1in)", "calc(96px)"),
        ("calc(1in + 2.54cm + 72pt)", "calc(288px)"),
        ("calc(2em - 1em)", "calc(1em)"),
        ("calc(50% - 10px)", "calc(50% - 10px)"),
        ("calc(10px - 50%)", "calc(-50% + 10px)"),
        ("calc(1px + 2em + 50%)", "calc(50% + 2em + 1px)"),
        ("calc((1px + 2em) * 2)", "calc(4em + 2px)"),
        ("calc(2 * (1em + 50%))", "calc(100% + 2em)"),
        ("calc(-1 * (1em - 2px))", "calc(-1em + 2px)"),
        ("calc(10px / 4)", "calc(2.5px)"),
        ("calc(1em * 2 / 4)", "calc(0.5em)"),
        ("calc(pi * 1px)", "calc(3.141593px)"),
        ("calc( ( ( 1px ) ) )", "calc(1px)"),
        ("calc(calc(1px + 1vw) * 3)", "calc(3px + 3vw)"),
        ("min(10px, 20px)", "calc(10px)"),
        ("min(10px, 5%)", "min(10px, 5%)"),
        ("max(1em, 2em, 10px)", "max(2em, 10px)"),
        ("clamp(1px, 2em, 3px)", "clamp(1px, 2em, 3px)"),
        ("clamp(10px, 5px, 20px)", "calc(10px)"),
        ("clamp(10px, 50px, 20px)", "calc(20px)"),
        ("calc(min(1px, 2em) + 3px)", "calc(3px + min(1px, 2em))"),
        ("calc(2 * min(1px, 2em))", "calc(2 * min(1px, 2em))"),
        ("calc(min(1px, 2em) / 2)", "calc(0.5 * min(1px, 2em))"),
        ("max(calc(50% - 1em), 0px)", "max(50% - 1em, 0px)"),
        ("calc(50%)", "calc(50%)"),
        ("calc(1px - 1px)", "calc(0px)"),
    ];
    let failures: Vec<_> = cases
        .iter()
        .filter_map(|&(input, want)| {
            let got = lp(input);
            (got.as_deref() != Some(want)).then(|| format!("{input:?}: got {got:?}, want {want:?}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn invalid_calculations_are_rejected() {
    for input in [
        "calc(1px+2px)",
        "calc(1px -2px)",
        "calc(1px- 2px)",
        "calc(1px - 2)",
        "calc(1 + 1px)",
        "calc(1px * 2px)",
        "calc(1px / 2px)",
        "calc(2 / 1px)",
        "calc(2)",
        "calc(e * 0 + 1px)",
        "calc()",
        "calc( )",
        "calc(1px, 2px)",
        "calc(1px 2px)",
        "calc(+ 1px)",
        "calc(1px +)",
        "calc(1px * )",
        "min()",
        "min(1px,)",
        "min(1px, 2)",
        "clamp(1px, 2px)",
        "clamp(1px, 2px, 3px, 4px)",
        "calc(1deg)",
        "calc(1s + 1px)",
        "calc(foo)",
        "calc(1px + [2px])",
        "calc({1px})",
        "round(1px, 2px)",
        "rgb(1px)",
        "var(--x)",
    ] {
        assert_eq!(lp(input), None, "{input:?}");
    }
    // Length-only contexts reject percentages.
    for input in ["calc(10%)", "calc(1px + 10%)", "min(1px, 10%)"] {
        assert!(Calc::parse_length(&value(input)).is_none(), "{input:?}");
    }
    let em = Calc::parse_length(&value("calc(1px + 0.5em)")).unwrap();
    assert_eq!(em.to_string(), "calc(0.5em + 1px)");
}

#[test]
fn number_calculations_evaluate_and_censor() {
    let n = |s: &str| Calc::parse_number(&value(s));
    assert_eq!(n("calc(1 + 2 * 3)"), Some(7.0));
    assert_eq!(n("calc((1 + 2) * 3)"), Some(9.0));
    assert_eq!(n("calc(10 / 4)"), Some(2.5));
    assert_eq!(n("calc(1 - 2 - 3)"), Some(-4.0));
    assert_eq!(n("min(1, 2, -3)"), Some(-3.0));
    assert_eq!(n("max(1, 2, -3)"), Some(2.0));
    assert_eq!(n("clamp(0, 5, 3)"), Some(3.0));
    assert_eq!(n("clamp(4, 5, 3)"), Some(4.0));
    assert_eq!(n("calc(1 / 0)"), Some(f64::MAX));
    assert_eq!(n("calc(-1 / 0)"), Some(-f64::MAX));
    assert_eq!(n("calc(infinity)"), Some(f64::MAX));
    assert_eq!(n("calc(-INFINITY)"), Some(-f64::MAX));
    assert_eq!(n("calc(NaN)"), Some(0.0));
    assert_eq!(n("calc(infinity - infinity)"), Some(0.0));
    assert_eq!(n("min(1, NaN)"), Some(0.0));
    assert_eq!(n("calc(e)"), Some(std::f64::consts::E));
    assert_eq!(n("calc(1px)"), None);
    assert_eq!(n("calc(50%)"), None);
    assert_eq!(n("calc(1 + 50%)"), None);
}

#[test]
fn calculations_resolve_against_lengths_and_percentages() {
    let calc = Calc::parse_length_percentage(&value("calc(50% - 2em + 1in)")).unwrap();
    assert!(calc.has_percentage());
    let em = |l: Length| match l.unit {
        LengthUnit::Em => l.value * 10.0,
        _ => l.to_px().unwrap(),
    };
    assert_eq!(calc.resolve(em, Some(200.0)), Some(100.0 - 20.0 + 96.0));
    assert_eq!(calc.resolve(em, None), None);
    assert_eq!(calc.compute(em).to_string(), "calc(50% + 76px)");
    let plain = Calc::parse_length_percentage(&value("max(2em, 3px)")).unwrap();
    assert!(!plain.has_percentage());
    assert_eq!(plain.compute(em).to_string(), "20px");
    // Infinite dimensions are censored when they are produced.
    let huge = Calc::parse_length_percentage(&value("calc(1px * infinity)")).unwrap();
    assert_eq!(huge.resolve(em, None), Some(f64::MAX));
    let mixed = Calc::parse_length_percentage(&value("calc(infinity * min(1em, 50%))")).unwrap();
    let computed = mixed.compute(em).to_string();
    assert!(!computed.contains("inf"), "{computed}");
}

#[test]
fn nesting_and_size_limits_are_exact() {
    let nested = |depth: usize| {
        let mut source = String::from("calc(");
        for _ in 1..depth {
            source.push('(');
        }
        source.push_str("1px");
        for _ in 1..depth {
            source.push(')');
        }
        source.push(')');
        source
    };
    assert!(lp(&nested(MAX_CALC_DEPTH)).is_some());
    assert!(lp(&nested(MAX_CALC_DEPTH + 1)).is_none());
    let mut functions = String::new();
    for _ in 0..MAX_CALC_DEPTH {
        functions.push_str("min(");
    }
    functions.push_str("1px");
    functions.push_str(&")".repeat(MAX_CALC_DEPTH));
    assert!(lp(&functions).is_some());
    let deeper = format!("calc({functions})");
    assert!(lp(&deeper).is_none());

    // A sum of n terms has 1 function + n values + (n - 1) operators.
    let sum = |terms: usize| format!("calc({})", vec!["1px"; terms].join(" + "));
    let max_terms = (MAX_CALC_NODES - 1).div_ceil(2);
    assert_eq!(
        lp(&sum(max_terms)).as_deref(),
        Some(format!("calc({max_terms}px)").as_str())
    );
    assert!(lp(&sum(max_terms + 1)).is_none());
}

#[test]
fn serialization_is_a_fixed_point() {
    for input in [
        "calc(1.0000001 * min(1px, 1em))",
        "calc(1em - 0.0000001px)",
        "calc(1em - 0.0000006px)",
        "calc(NaN * min(1px, 1em))",
        "calc(infinity * max(1px, 50%))",
        "calc(1px - min(1px, 1em))",
        "calc(-1 * min(1px, 1em) + max(1em, 1px))",
        "calc(min(1px, 1em) / 3)",
        "clamp(1em - 1px, 50%, 3px * 2)",
        "calc(1px / 0)",
        "calc(0.0000001 * min(1px, 1em))",
    ] {
        let once = lp(input).unwrap_or_else(|| panic!("{input:?} did not parse"));
        let again = lp(&once);
        assert_eq!(again.as_deref(), Some(once.as_str()), "{input:?}");
    }
}

#[test]
fn mix_weights_and_simplifies_length_percentages() {
    use rustic_css::{Calc, Length, LengthPercentage};
    let px = |v| LengthPercentage::Length(Length::px(v));
    assert_eq!(Calc::mix(&px(10.0), 0.75, &px(30.0), 0.25), px(15.0));
    assert_eq!(
        Calc::mix(
            &LengthPercentage::Percentage(20.0),
            0.5,
            &LengthPercentage::Percentage(40.0),
            0.5
        ),
        LengthPercentage::Percentage(30.0)
    );
    let mixed = Calc::mix(&px(10.0), 0.5, &LengthPercentage::Percentage(50.0), 0.5);
    assert_eq!(mixed.to_string(), "calc(25% + 5px)");
    assert_eq!(
        Calc::mix(&mixed, 1.0, &px(0.0), 0.0).to_string(),
        "calc(25% + 5px)"
    );
}
