//! Media query parsing, serialization and evaluation (CSS-06).

use rustic_css::{
    ColorScheme, CssParser, FontUnitRatios, MAX_MEDIA_NESTING, MediaEnvironment, MediaQueryList,
    MediaType, PointerAccuracy,
};

fn parse(source: &str) -> MediaQueryList {
    CssParser::default().media_query_list(source).unwrap().value
}

fn serialized(source: &str) -> String {
    parse(source).to_string()
}

#[test]
fn serialization_is_canonical_and_a_fixed_point() {
    for (source, expected) in [
        ("", ""),
        ("  ", ""),
        ("all", "all"),
        ("SCREEN", "screen"),
        ("only screen", "only screen"),
        ("not print", "not print"),
        ("tv", "tv"),
        (
            "screen and (min-width: 40em)",
            "screen and (min-width: 40em)",
        ),
        ("(MAX-WIDTH:600PX)", "(max-width: 600px)"),
        ("(color)", "(color)"),
        ("(width >= 600px)", "(width >= 600px)"),
        ("(600px<width)", "(width > 600px)"),
        ("(400px <= width < 700px)", "(400px <= width < 700px)"),
        ("(700px > width >= 400px)", "(700px > width >= 400px)"),
        ("(aspect-ratio: 16/9)", "(aspect-ratio: 16 / 9)"),
        ("(min-aspect-ratio: 2)", "(min-aspect-ratio: 2 / 1)"),
        ("(resolution >= 2dppx)", "(resolution >= 2dppx)"),
        ("(min-resolution: 192DPI)", "(min-resolution: 192dpi)"),
        ("(hover: HOVER)", "(hover: hover)"),
        ("(grid: 0)", "(grid: 0)"),
        ("not (color)", "not (color)"),
        ("(color) and (hover)", "(color) and (hover)"),
        ("(color) or (not (hover))", "(color) or (not (hover))"),
        ("((color))", "(color)"),
        (
            "screen and ((color) or (hover))",
            "screen and ((color) or (hover))",
        ),
        (
            "print, screen and (orientation: landscape)",
            "print, screen and (orientation: landscape)",
        ),
        // General-enclosed: kept, and unknown at evaluation.
        ("(unknown-feature)", "(unknown-feature)"),
        ("(width: red)", "(width: red)"),
        ("(min-orientation: portrait)", "(min-orientation: portrait)"),
        ("foo(bar)", "foo(bar)"),
        ("(color) and (foo  bar)", "(color) and (foo bar)"),
        // Invalid queries become `not all`; the rest of the list survives.
        ("screen and", "not all"),
        ("only", "not all"),
        ("not", "not all"),
        ("and (color)", "not all"),
        ("screen (color)", "not all"),
        ("(color) and (hover) or (grid)", "not all"),
        ("screen and (color) or (hover)", "not all"),
        ("layer", "not all"),
        ("print, , screen", "print, not all, screen"),
        ("(width < 1px > 2px)", "(width < 1px > 2px)"),
        ("(width < = 1px)", "(width < = 1px)"),
        ("\"x\"", "not all"),
        ("(bad \"\n)", "not all"),
    ] {
        let got = serialized(source);
        assert_eq!(got, expected, "{source:?}");
        assert_eq!(serialized(&got), got, "fixed point of {source:?}");
    }
}

#[test]
fn deep_nesting_stays_bounded() {
    let depth = MAX_MEDIA_NESTING + 8;
    let source = format!("{}(color){}", "(".repeat(depth), ")".repeat(depth));
    let list = parse(&source);
    assert_eq!(list.queries.len(), 1);
    assert!(!list.matches(&MediaEnvironment::default()));
    let shallow = format!("{}(color){}", "(".repeat(8), ")".repeat(8));
    assert_eq!(serialized(&shallow), "(color)");
    assert!(parse(&shallow).matches(&MediaEnvironment::default()));
}

#[test]
fn evaluation_follows_the_environment() {
    let desktop = MediaEnvironment::default();
    let mut phone = MediaEnvironment::screen(390.0, 844.0);
    phone.resolution = 3.0;
    phone.pointer = PointerAccuracy::Coarse;
    phone.any_pointer = PointerAccuracy::Coarse;
    phone.hover = false;
    phone.any_hover = false;
    phone.color_scheme = ColorScheme::Dark;
    phone.reduced_motion = true;
    let mut print = MediaEnvironment::screen(816.0, 1056.0);
    print.media_type = MediaType::Print;

    for (source, on_desktop, on_phone, on_print) in [
        ("", true, true, true),
        ("all", true, true, true),
        ("screen", true, true, false),
        ("print", false, false, true),
        ("only screen", true, true, false),
        ("not print", true, true, false),
        ("tv", false, false, false),
        ("not tv", true, true, true),
        ("(min-width: 600px)", true, false, true),
        ("(max-width: 600px)", false, true, false),
        ("(width: 1280px)", true, false, false),
        (
            "(width > 1279.5px) and (width < 1280.5px)",
            true,
            false,
            false,
        ),
        ("(400px <= width <= 900px)", false, false, true),
        ("(min-width: 50em)", true, false, true),
        ("(width >= 100vh)", true, false, false),
        ("(orientation: portrait)", false, true, true),
        ("(orientation: landscape)", true, false, false),
        ("(min-aspect-ratio: 16/9)", true, false, false),
        ("(aspect-ratio: 16/9)", true, false, false),
        ("(max-aspect-ratio: 1)", false, true, true),
        ("(min-resolution: 2dppx)", false, true, false),
        ("(resolution: 96dpi)", true, false, true),
        ("(color)", true, true, true),
        ("(min-color: 8)", true, true, true),
        ("(monochrome)", false, false, false),
        ("(grid)", false, false, false),
        ("(grid: 0)", true, true, true),
        ("(hover)", true, false, true),
        ("(hover: none)", false, true, false),
        ("(pointer: coarse)", false, true, false),
        ("(any-pointer: fine)", true, false, true),
        ("(prefers-color-scheme: dark)", false, true, false),
        ("(prefers-reduced-motion)", false, true, false),
        ("(prefers-reduced-motion: no-preference)", true, false, true),
        ("(scripting: none)", true, true, true),
        ("(update: fast)", true, true, false),
        ("(overflow-block: paged)", false, false, true),
        ("not (hover)", false, true, false),
        ("(hover) or (min-resolution: 2x)", true, true, true),
        ("screen and (not (hover))", false, true, false),
        ("print, (max-width: 400px)", false, true, true),
        // Unknown never matches, and `not` keeps it unknown.
        ("(unknown)", false, false, false),
        ("not (unknown)", false, false, false),
        // The media type fails first in print, so `not` makes it true.
        ("not screen and (unknown)", false, false, true),
        ("(unknown) or (color)", true, true, true),
        ("(unknown) and (grid)", false, false, false),
        ("not all", false, false, false),
        ("screen and", false, false, false),
    ] {
        let list = parse(source);
        assert_eq!(list.matches(&desktop), on_desktop, "{source:?} on desktop");
        assert_eq!(list.matches(&phone), on_phone, "{source:?} on phone");
        assert_eq!(list.matches(&print), on_print, "{source:?} in print");
    }
}

#[test]
fn font_relative_units_use_the_initial_font_ratios() {
    // Fallbacks: 1ex = 8px, 1cap = 11.2px, 1ic = 16px at the 16px initial font.
    let mut env = MediaEnvironment::screen(400.0, 300.0);
    assert!(parse("(width = 50ex)").matches(&env));
    assert!(parse("(width = 25ic)").matches(&env));
    // A loaded initial font with a 0.25em x-height, 0.625em `0` advance,
    // 0.5em cap height and 1.25em ideographic advance.
    env.initial_font = FontUnitRatios {
        ex: 0.25,
        ch: 0.625,
        cap: 0.5,
        ic: 1.25,
    };
    for (source, want) in [
        ("(width = 100ex)", true),
        ("(width = 50ex)", false),
        ("(width = 40ch)", true),
        ("(width = 40rch)", true),
        ("(width = 50cap)", true),
        ("(width = 20ic)", true),
        ("(width = 25em)", true),
    ] {
        assert_eq!(parse(source).matches(&env), want, "{source}");
    }
}
