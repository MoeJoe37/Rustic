//! REN-02c: intrinsic sizing keywords (CSS Sizing 3 §3–§5) in the block
//! reference, and flex-item blockification from M2.

use rustic_base::CancellationToken;
use rustic_css::{Direction, Display, PropertyId};
use rustic_layout::{
    BlockLayout, BoxKind, BoxTree, CssPx, CssSize, LayoutLimits, MeasureError, measure_block_width,
};

mod common;

const RESET: &str = "* { margin:0; }";

fn tree(html: &str, css: &str) -> BoxTree {
    let (doc, styles) = common::page(html, &format!("{RESET} {css}"));
    BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap()
}

fn layout(html: &str, css: &str) -> BlockLayout {
    BlockLayout::compute(
        &tree(html, css),
        CssSize::new(300.0, 200.0).unwrap(),
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap()
}

/// Content-box width of fragment `i`.
fn width(l: &BlockLayout, i: usize) -> f64 {
    l.fragments()[i].content().size().width().get()
}

/// Border-box x of fragment `i`.
fn x(l: &BlockLayout, i: usize) -> f64 {
    l.fragments()[i].border().origin().x.get()
}

// Fragment indexes: 0 html, 1 body, 2 main, 3 its first child, ...
// Children: one 40px-wide block, and a float pair (60px and 50px) whose
// min-content is 60 and max-content 110.
const CONTENT: &str = "<main><div id=a></div><div id=f1></div><div id=f2></div></main>";
const ITEMS: &str = "#a {width:40px;height:5px} #f1 {float:left;width:60px;height:5px} \
                     #f2 {float:left;width:50px;height:5px}";

#[test]
fn width_keywords_use_min_and_max_content() {
    for (rule, expected) in [
        ("width:min-content", 60.0),
        ("width:max-content", 110.0),
        ("width:fit-content", 110.0),
        ("width:fit-content(80px)", 80.0),
        ("width:fit-content(10px)", 60.0),
        ("width:fit-content(500px)", 110.0),
        ("width:fit-content(50%)", 150.0f64.min(110.0)),
        // Border-box sizing does not change a keyword's content box.
        (
            "width:max-content;box-sizing:border-box;padding:0 5px",
            110.0,
        ),
        ("width:10px;min-width:max-content", 110.0),
        ("width:auto;max-width:min-content", 60.0),
        ("width:500px;max-width:fit-content", 110.0),
    ] {
        let l = layout(CONTENT, &format!("{ITEMS} main {{{rule}}}"));
        assert_eq!(width(&l, 2), expected, "{rule}");
    }
}

#[test]
fn fit_content_in_a_narrow_containing_block_shrinks_to_fit() {
    // 80px available: between min-content (60) and max-content (110).
    let l = layout(
        &format!("<section>{CONTENT}</section>"),
        &format!("{ITEMS} section {{width:80px}} main {{width:fit-content}}"),
    );
    assert_eq!(width(&l, 3), 80.0);
    // Auto margins centre a keyword-sized block like any fixed width.
    let l = layout(
        CONTENT,
        &format!("{ITEMS} main {{width:max-content;margin:0 auto}}"),
    );
    assert_eq!(x(&l, 2), 95.0);
}

#[test]
fn keyword_widths_propagate_through_intrinsic_contributions() {
    // A float sized by shrink-to-fit sees a child whose width is max-content.
    let l = layout(
        "<main><div id=wrap><div id=inner><div id=f1></div><div id=f2></div></div></div></main>",
        "#wrap {float:left} #inner {width:max-content} \
         #f1 {float:left;width:60px;height:5px} #f2 {float:left;width:50px;height:5px}",
    );
    assert_eq!(width(&l, 3), 110.0);
    // An absolutely positioned box with fit-content shrink-wraps too.
    let l = layout(
        CONTENT,
        &format!("{ITEMS} main {{position:absolute;left:0;width:fit-content}}"),
    );
    assert_eq!(width(&l, 2), 110.0);
}

#[test]
fn block_axis_keywords_behave_as_their_initial_values() {
    let l = layout(
        "<main><div id=a></div></main>",
        "#a {height:30px} main {height:max-content;min-height:min-content;max-height:fit-content}",
    );
    assert_eq!(l.fragments()[2].content().size().height().get(), 30.0);
}

#[test]
fn the_public_width_measure_reports_missing_content_sizes() {
    let (_, styles) = common::page("<div id=a></div>", "#a {width:min-content}");
    let style = &styles
        .iter()
        .find(|s| {
            matches!(
                s.computed.get(PropertyId::Width),
                rustic_css::PropertyValue::Size(rustic_css::Size::MinContent)
            )
        })
        .unwrap()
        .computed;
    assert_eq!(
        measure_block_width(style, CssPx::new(100.0).unwrap(), Direction::Ltr),
        Err(MeasureError::IntrinsicSizeRequired)
    );
}

#[test]
fn flex_children_are_blockified_but_contents_children_are_not() {
    let t = tree(
        "<main><span id=a></span><em id=b></em><b id=c></b></main>",
        "main {display:flex} #b {display:inline-block} #c {display:contents}",
    );
    let kinds: Vec<_> = t.boxes().iter().map(|b| b.kind()).collect();
    // html, body, main, then span and em blockified; the contents box is gone.
    assert_eq!(
        kinds[2..],
        [
            BoxKind::Element(Display::Flex),
            BoxKind::Element(Display::Block),
            BoxKind::Element(Display::Block),
        ]
    );
    let t = tree(
        "<main><span></span></main>",
        "main {display:inline-flex;float:left}",
    );
    assert_eq!(t.boxes()[3].kind(), BoxKind::Element(Display::Block));
}
