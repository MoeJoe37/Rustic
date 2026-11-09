use rustic_base::CancellationToken;
use rustic_css::{Display, PropertyId as P};
use rustic_dom::{DomString, NodeKind};
use rustic_layout::{
    BoxKind, BoxTree, FormattingError as E, FormattingKind as K, FormattingLimits, FormattingTree,
    InlineEdges, LayoutLimits,
};
mod common;
use common::page;

fn principal(html: &str, css: &str) -> BoxTree {
    let (doc, styles) = page(&format!("<main>{html}</main>"), css);
    let root = doc.walk(doc.root()).unwrap().map(|(id, _)| id).find(|&id|
        matches!(doc.node(id).unwrap().kind(), NodeKind::Element(e) if e.name().local() == "main")
    ).unwrap();
    BoxTree::build(
        &doc,
        root,
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap()
}
fn normalize(p: &BoxTree) -> FormattingTree<'_> {
    FormattingTree::build(
        p,
        FormattingLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap()
}
fn kinds(t: &FormattingTree<'_>) -> Vec<(K, Option<usize>)> {
    t.boxes().iter().map(|b| (b.kind(), b.parent())).collect()
}
const BLOCK: K = K::Block(Display::Block);

#[test]
fn pure_inline_context_needs_no_anonymous_block() {
    let p = principal("a<span>b</span>c", "");
    let t = normalize(&p);
    assert_eq!(
        kinds(&t),
        vec![
            (BLOCK, None),
            (K::Text, Some(0)),
            (K::Inline, Some(0)),
            (K::Text, Some(2)),
            (K::Text, Some(0))
        ]
    );
    assert!(t.suppressed_whitespace().is_empty());
    assert!(std::ptr::eq(t.principal(), &p));
}

#[test]
fn mixed_runs_get_minimal_wrappers_and_source_mapping() {
    let p = principal(
        "a<span>b</span><div>c</div>d<div></div>",
        "main { color: blue; background: red; width: 80px; padding: 4px; border: 2px solid }",
    );
    let t = normalize(&p);
    assert_eq!(
        kinds(&t),
        vec![
            (BLOCK, None),
            (K::AnonymousBlock, Some(0)),
            (K::Text, Some(1)),
            (K::Inline, Some(1)),
            (K::Text, Some(3)),
            (BLOCK, Some(0)),
            (K::Text, Some(5)),
            (K::AnonymousBlock, Some(0)),
            (K::Text, Some(7)),
            (BLOCK, Some(0))
        ]
    );
    for i in [1, 7] {
        let b = &t.boxes()[i];
        assert_eq!(b.source(), None);
        assert_eq!(b.style_source(), 0);
        let s = t.style(i).unwrap();
        assert_eq!(s.get(P::Color).to_string(), "rgb(0, 0, 255)");
        assert_eq!(s.get(P::BackgroundColor).to_string(), "rgba(0, 0, 0, 0)");
        assert_eq!(s.get(P::Width).to_string(), "auto");
        assert_eq!(s.get(P::PaddingLeft).to_string(), "0px");
        assert_eq!(s.get(P::BorderLeftWidth).to_string(), "0px");
    }
    assert!(t.style(usize::MAX).is_none());
    assert!(t.text(usize::MAX).is_empty());
}

#[test]
fn nested_inline_ancestors_split_and_keep_open_edges() {
    let p = principal("<span>a<em>b<div>c</div>d</em>e</span>", "");
    let t = normalize(&p);
    assert_eq!(
        kinds(&t),
        vec![
            (BLOCK, None),
            (K::AnonymousBlock, Some(0)),
            (K::Inline, Some(1)),
            (K::Text, Some(2)),
            (K::Inline, Some(2)),
            (K::Text, Some(4)),
            (BLOCK, Some(0)),
            (K::Text, Some(6)),
            (K::AnonymousBlock, Some(0)),
            (K::Inline, Some(8)),
            (K::Inline, Some(9)),
            (K::Text, Some(10)),
            (K::Text, Some(9))
        ]
    );
    for (first, last) in [(2, 9), (4, 10)] {
        assert_eq!(t.boxes()[first].source(), t.boxes()[last].source());
        assert_eq!(
            t.boxes()[first].inline_edges(),
            Some(InlineEdges {
                start: true,
                end: false
            })
        );
        assert_eq!(
            t.boxes()[last].inline_edges(),
            Some(InlineEdges {
                start: false,
                end: true
            })
        );
    }
    assert_eq!(
        (0..t.boxes().len())
            .flat_map(|i| t.text(i).iter().copied())
            .collect::<Vec<_>>(),
        "abcde".encode_utf16().collect::<Vec<_>>()
    );
}

#[test]
fn consecutive_block_group_keeps_only_empty_leading_and_trailing_continuations() {
    for separator in ["", " \n\t"] {
        let p = principal(
            &format!("<span><div></div>{separator}<div></div></span>"),
            "",
        );
        let t = normalize(&p);
        assert_eq!(
            kinds(&t),
            vec![
                (BLOCK, None),
                (K::AnonymousBlock, Some(0)),
                (K::Inline, Some(1)),
                (BLOCK, Some(0)),
                (BLOCK, Some(0)),
                (K::AnonymousBlock, Some(0)),
                (K::Inline, Some(5))
            ]
        );
        assert_eq!(
            t.suppressed_whitespace().len(),
            usize::from(!separator.is_empty())
        );
    }
}

#[test]
fn separate_block_groups_create_a_middle_continuation_and_keep_empty_inline_elements() {
    let p = principal(
        "<span><div></div>x<div></div><em></em><div></div></span>",
        "",
    );
    let t = normalize(&p);
    let span = t
        .boxes()
        .iter()
        .filter(|b| b.source() == Some(1))
        .collect::<Vec<_>>();
    assert_eq!(span.len(), 4);
    assert_eq!(
        span.iter()
            .map(|b| b.inline_edges().unwrap())
            .collect::<Vec<_>>(),
        vec![
            InlineEdges {
                start: true,
                end: false
            },
            InlineEdges {
                start: false,
                end: false
            },
            InlineEdges {
                start: false,
                end: false
            },
            InlineEdges {
                start: false,
                end: true
            }
        ]
    );
    assert_eq!(
        t.boxes()
            .iter()
            .filter(|b| b.kind() == K::AnonymousBlock)
            .count(),
        4
    );
    assert_eq!(
        t.boxes().iter().filter(|b| b.kind() == K::Inline).count(),
        5
    );
}

#[test]
fn inline_block_is_atomic_outside_and_normalized_inside() {
    let p = principal(
        "a<span>x<div></div>y</span>b<div></div>",
        "span { display:inline-block }",
    );
    let t = normalize(&p);
    let atomic = t
        .boxes()
        .iter()
        .position(|b| b.kind() == K::InlineBlock)
        .unwrap();
    assert_eq!(t.boxes()[atomic].parent(), Some(1));
    assert_eq!(
        t.boxes()
            .iter()
            .filter(|b| b.parent() == Some(atomic))
            .map(|b| b.kind())
            .collect::<Vec<_>>(),
        vec![K::AnonymousBlock, BLOCK, K::AnonymousBlock]
    );
    assert_eq!(
        t.boxes()
            .iter()
            .filter(|b| b.parent() == Some(0))
            .map(|b| b.kind())
            .collect::<Vec<_>>(),
        vec![K::AnonymousBlock, BLOCK]
    );
}

#[test]
fn only_collapsible_ascii_whitespace_between_blocks_is_omitted() {
    for white_space in [
        "normal",
        "nowrap",
        "pre",
        "pre-wrap",
        "pre-line",
        "break-spaces",
    ] {
        let p = principal(
            " \t<div></div> \n<div></div> ",
            &format!("main {{ white-space:{white_space} }}"),
        );
        let t = normalize(&p);
        let collapse = matches!(white_space, "normal" | "nowrap");
        assert_eq!(
            t.suppressed_whitespace().len(),
            if collapse { 3 } else { 0 },
            "{white_space}"
        );
        assert_eq!(
            t.boxes()
                .iter()
                .filter(|b| b.kind() == K::AnonymousBlock)
                .count(),
            if collapse { 0 } else { 3 }
        );
        for &i in t.suppressed_whitespace() {
            assert!(
                p.boxes()[i]
                    .text()
                    .iter()
                    .all(|u| matches!(u, 9 | 10 | 12 | 13 | 32))
            );
        }
    }
    let p = principal("<div></div>\u{a0}<div></div>\u{3000}", "");
    let t = normalize(&p);
    assert!(t.suppressed_whitespace().is_empty());
    assert_eq!(
        t.boxes()
            .iter()
            .filter(|b| b.kind() == K::AnonymousBlock)
            .count(),
        2
    );
}

#[test]
fn contents_and_hidden_nodes_keep_dom_inheritance_before_normalization() {
    let p = principal(
        "<section>x<div></div>y</section><aside><img></aside><template><img></template>",
        "section {display:contents;color:blue;background:red} aside {display:none}",
    );
    let t = normalize(&p);
    assert_eq!(
        t.boxes()
            .iter()
            .filter(|b| b.kind() == K::AnonymousBlock)
            .count(),
        2
    );
    let i = t.boxes().iter().position(|b| b.kind() == K::Text).unwrap();
    assert_eq!(
        t.style(i).unwrap().get(P::Color).to_string(),
        "rgb(0, 0, 255)"
    );
    assert_eq!(
        t.style(i).unwrap().get(P::BackgroundColor).to_string(),
        "rgba(0, 0, 0, 0)"
    );
}

#[test]
fn unpaired_surrogates_and_multilingual_units_survive_unchanged() {
    let (mut doc, styles) = page("<main>text<div></div></main>", "");
    let root = styles.iter().find(|s| matches!(doc.node(s.node).unwrap().kind(), NodeKind::Element(e) if e.name().local() == "main")).unwrap().node;
    let text = doc
        .walk(root)
        .unwrap()
        .find(|&(id, _)| matches!(doc.node(id).unwrap().kind(), NodeKind::Text(_)))
        .unwrap()
        .0;
    let units = vec![0xd800, 0x41, 0x0628, 0x4e2d, 0xd83d, 0xde00, 0xdc00];
    doc.set_character_data(text, DomString::from_units(units.clone()))
        .unwrap();
    let p = BoxTree::build(
        &doc,
        root,
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let before = p.clone();
    let t = normalize(&p);
    assert_eq!(t.text(2), &units);
    assert!(!t.dump(10000).unwrap().contains("text="));
    assert_eq!(p, before);
}

#[test]
fn anonymous_styles_resolve_currentcolor_and_registered_inheritance() {
    let p = principal(
        "x<div></div>",
        r#"
        @property --local { syntax: '<length>'; inherits: false; initial-value: 3px; }
        @property --inherited { syntax: '<length>'; inherits: true; initial-value: 4px; }
        main { color:blue; --local:9px; --inherited:10px; --ordinary:hello; font-size:20px; line-height:1.5; text-indent:2em; border: 5px solid red }
    "#,
    );
    let t = normalize(&p);
    for i in [1, 2] {
        let s = t.style(i).unwrap();
        assert_eq!(s.get(P::BorderLeftColor), s.get(P::Color));
        assert_eq!(s.get(P::FontSize).to_string(), "20px");
        assert_eq!(s.get(P::LineHeight).to_string(), "1.5");
        assert_eq!(s.get(P::TextIndent).to_string(), "40px");
        assert_eq!(
            s.custom_properties().get("--local").unwrap().to_string(),
            "3px"
        );
        assert_eq!(
            s.custom_properties()
                .get("--inherited")
                .unwrap()
                .to_string(),
            "10px"
        );
        assert_eq!(
            s.custom_properties().get("--ordinary").unwrap().to_string(),
            "hello"
        );
    }
    assert_eq!(
        p.boxes()[0]
            .style()
            .unwrap()
            .custom_properties()
            .get("--local")
            .unwrap()
            .to_string(),
        "9px"
    );
}

#[test]
fn sibling_text_runs_share_one_parent_and_one_anonymous_style() {
    // Text runs must not each copy a multi-kilobyte style: siblings share
    // their parent's snapshot style and one derived anonymous inline style.
    let p = principal("a<span></span>b<b>c</b>d", "");
    let texts: Vec<usize> = (0..p.boxes().len())
        .filter(|&i| p.boxes()[i].kind() == BoxKind::Text && p.boxes()[i].parent() == Some(0))
        .collect();
    assert_eq!(texts.len(), 3);
    let parent = p.boxes()[0].style().unwrap();
    for &i in &texts {
        assert!(std::ptr::eq(p.boxes()[i].style().unwrap(), parent));
    }
    let nested = p
        .boxes()
        .iter()
        .position(|b| b.kind() == BoxKind::Text && b.parent() != Some(0))
        .unwrap();
    assert!(!std::ptr::eq(p.boxes()[nested].style().unwrap(), parent));
    let t = normalize(&p);
    let anonymous: Vec<_> = (0..t.boxes().len())
        .filter(|&i| {
            t.boxes()[i].kind() == K::Text && texts.contains(&t.boxes()[i].source().unwrap())
        })
        .map(|i| t.style(i).unwrap())
        .collect();
    assert_eq!(anonymous.len(), 3);
    assert!(anonymous.iter().all(|s| std::ptr::eq(*s, anonymous[0])));
    assert_eq!(anonymous[0].get(P::Display).to_string(), "inline");
}

#[test]
fn unsupported_formatting_fails_without_mutating_the_snapshot() {
    for html in [
        "<img>",
        "<input>",
        "<br>",
        "<svg></svg>",
        "<table><tr><td></td></tr></table>",
        "<li></li>",
    ] {
        let p = principal(html, "");
        let before = p.clone();
        assert!(
            matches!(
                FormattingTree::build(
                    &p,
                    FormattingLimits::default(),
                    &CancellationToken::default()
                ),
                Err(E::Unsupported { .. })
            ),
            "{html}"
        );
        assert_eq!(p, before);
    }
    for css in [
        "display:flex",
        "display:grid",
        "position:relative",
        "position:absolute",
        "position:fixed",
        "position:sticky",
        "float:left",
    ] {
        let p = principal("<span></span>", &format!("span {{ {css} }}"));
        assert!(
            matches!(
                FormattingTree::build(
                    &p,
                    FormattingLimits::default(),
                    &CancellationToken::default()
                ),
                Err(E::Unsupported { .. })
            ),
            "{css}"
        );
    }
}

fn result(p: &BoxTree, limits: FormattingLimits) -> Result<usize, E> {
    FormattingTree::build(p, limits, &CancellationToken::default()).map(|t| t.boxes().len())
}
#[test]
fn exact_box_depth_work_and_dump_budgets_and_cancellation() {
    let p = principal("a<div></div>b", "");
    let limits = FormattingLimits::default();
    assert_eq!(
        result(
            &p,
            FormattingLimits {
                max_boxes: 6,
                ..limits
            }
        ),
        Ok(6)
    );
    assert_eq!(
        result(
            &p,
            FormattingLimits {
                max_boxes: 5,
                ..limits
            }
        ),
        Err(E::BoxLimit)
    );
    assert_eq!(
        result(
            &p,
            FormattingLimits {
                max_depth: 2,
                ..limits
            }
        ),
        Ok(6)
    );
    assert_eq!(
        result(
            &p,
            FormattingLimits {
                max_depth: 1,
                ..limits
            }
        ),
        Err(E::DepthLimit)
    );
    let first_ok = (0..1000)
        .find(|&n| {
            result(
                &p,
                FormattingLimits {
                    max_steps: n,
                    ..limits
                },
            )
            .is_ok()
        })
        .unwrap();
    assert_eq!(
        result(
            &p,
            FormattingLimits {
                max_steps: first_ok - 1,
                ..limits
            }
        ),
        Err(E::WorkLimit)
    );
    assert_eq!(
        result(
            &p,
            FormattingLimits {
                max_steps: usize::MAX,
                ..limits
            }
        ),
        Ok(6)
    );
    let t = normalize(&p);
    let dump = t.dump(10000).unwrap();
    assert_eq!(t.dump(dump.len()), Ok(dump.clone()));
    assert_eq!(t.dump(dump.len() - 1), Err(E::OutputLimit));
    let token = CancellationToken::default();
    token.cancel();
    assert_eq!(
        FormattingTree::build(&p, limits, &token).unwrap_err(),
        E::Cancelled
    );
}

#[test]
fn empty_snapshot_and_zero_limits_are_well_defined() {
    let p = principal("<img>", "main {display:none}");
    assert!(p.boxes().is_empty());
    let limits = FormattingLimits {
        max_boxes: 0,
        max_depth: 0,
        max_steps: 0,
    };
    let t = FormattingTree::build(&p, limits, &CancellationToken::default()).unwrap();
    assert!(t.boxes().is_empty());
    assert_eq!(t.dump(0).unwrap(), "");
    let p = principal("", "");
    assert_eq!(
        result(
            &p,
            FormattingLimits {
                max_boxes: 0,
                ..FormattingLimits::default()
            }
        ),
        Err(E::BoxLimit)
    );
    assert_eq!(
        result(
            &p,
            FormattingLimits {
                max_depth: 0,
                ..FormattingLimits::default()
            }
        ),
        Ok(1)
    );
}

#[test]
fn deep_inline_splits_are_iterative_and_expansion_is_bounded() {
    let p = principal(
        &format!(
            "{}x<div></div>y{}",
            "<span>".repeat(180),
            "</span>".repeat(180)
        ),
        "",
    );
    let t = normalize(&p);
    assert_eq!(
        t.boxes().iter().filter(|b| b.kind() == K::Inline).count(),
        360
    );
    assert_eq!(
        result(
            &p,
            FormattingLimits {
                max_boxes: 200,
                ..FormattingLimits::default()
            }
        ),
        Err(E::BoxLimit)
    );
    assert_eq!(
        result(
            &p,
            FormattingLimits {
                max_depth: 100,
                ..FormattingLimits::default()
            }
        ),
        Err(E::DepthLimit)
    );
    assert_eq!(
        result(
            &p,
            FormattingLimits {
                max_steps: 400,
                ..FormattingLimits::default()
            }
        ),
        Err(E::WorkLimit)
    );
}

#[test]
fn exhaustive_short_runs_match_an_independent_block_group_model() {
    // A token model describes the expected *top-level* groups independently
    // of the production bottom-up arena. Every block mask through six tokens,
    // with up to three wrapping inline ancestors, exercises splitting/wrapping.
    for len in 0..=6 {
        for mask in 0..(1usize << len) {
            let tokens = (0..len)
                .map(|i| {
                    if mask & (1 << i) != 0 {
                        "<div></div>"
                    } else {
                        "x"
                    }
                })
                .collect::<String>();
            for depth in 0..=3 {
                let p = principal(
                    &format!(
                        "{}{tokens}{}",
                        "<span>".repeat(depth),
                        "</span>".repeat(depth)
                    ),
                    "",
                );
                let t = normalize(&p);
                let mut expected = Vec::new();
                if mask == 0 {
                    if depth > 0 {
                        expected.push(K::Inline);
                    } else if len > 0 {
                        expected.push(K::Text);
                    }
                } else {
                    if depth > 0 || mask & 1 == 0 {
                        expected.push(K::AnonymousBlock);
                    }
                    for i in 0..len {
                        if mask & (1 << i) != 0 {
                            expected.push(BLOCK);
                            if i + 1 < len && mask & (1 << (i + 1)) == 0 {
                                expected.push(K::AnonymousBlock);
                            }
                        }
                    }
                    if depth > 0 && mask & (1 << (len - 1)) != 0 {
                        expected.push(K::AnonymousBlock);
                    }
                }
                assert_eq!(
                    t.boxes()
                        .iter()
                        .filter(|b| b.parent() == Some(0))
                        .map(|b| b.kind())
                        .collect::<Vec<_>>(),
                    expected,
                    "len={len} mask={mask} depth={depth}"
                );
                for (i, b) in t.boxes().iter().enumerate() {
                    let child_kinds = t
                        .boxes()
                        .iter()
                        .filter(|c| c.parent() == Some(i))
                        .map(|c| c.kind())
                        .collect::<Vec<_>>();
                    let blocks = child_kinds
                        .iter()
                        .filter(|k| matches!(k, K::Block(_) | K::AnonymousBlock))
                        .count();
                    assert!(blocks == 0 || blocks == child_kinds.len());
                    if b.kind() == K::Inline {
                        assert_eq!(blocks, 0);
                    }
                    if let Some(parent) = b.parent() {
                        assert!(parent < i);
                    }
                }
                let original = p
                    .boxes()
                    .iter()
                    .filter(|b| b.kind() == BoxKind::Text)
                    .flat_map(|b| b.text().iter().copied())
                    .collect::<Vec<_>>();
                let normalized = (0..t.boxes().len())
                    .flat_map(|i| t.text(i).iter().copied())
                    .collect::<Vec<_>>();
                assert_eq!(original, normalized);
            }
        }
    }
}

#[test]
fn repeated_block_groups_bound_actual_continuation_expansion() {
    let p = principal(
        &format!(
            "{}{}{}",
            "<span>".repeat(60),
            "x<div></div>".repeat(64),
            "</span>".repeat(60)
        ),
        "",
    );
    let t = normalize(&p);
    assert!(t.boxes().len() > 20 * p.boxes().len());
    assert_eq!(
        result(
            &p,
            FormattingLimits {
                max_steps: p.boxes().len() * 10,
                ..FormattingLimits::default()
            }
        ),
        Err(E::WorkLimit)
    );
    assert_eq!(
        result(
            &p,
            FormattingLimits {
                max_boxes: 1000,
                ..FormattingLimits::default()
            }
        ),
        Err(E::BoxLimit)
    );
    let p = principal("<div></div> <div></div>", "");
    assert_eq!(normalize(&p).boxes().len(), 3);
    assert_eq!(
        result(
            &p,
            FormattingLimits {
                max_boxes: 3,
                ..FormattingLimits::default()
            }
        ),
        Err(E::BoxLimit)
    );
}
