//! The general fragment tree over block, line, inline-box and text
//! fragments (`layout.boxes` fragments step).

use std::collections::HashSet;
use std::sync::Arc;

use rustic_base::CancellationToken;
use rustic_layout::{BlockLayout, BoxTree, CssSize, FlowError, FragmentKind, LayoutLimits};
use rustic_style::ComputeContext;
use rustic_text::bundled::test_database;

mod common;

const RESET: &str = "* { margin:0; font-size:10px; } .ib { display:inline-block } ";

fn layout(html: &str, css: &str) -> BlockLayout {
    let fonts = test_database();
    let context = ComputeContext::with_fonts(Arc::new(test_database()));
    let (doc, styles) = common::page_with_context(html, &format!("{RESET}{css}"), context);
    let tree = BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    BlockLayout::compute_with_fonts(
        &tree,
        CssSize::new(100.0, 400.0).unwrap(),
        LayoutLimits::default(),
        &fonts,
        &CancellationToken::default(),
    )
    .unwrap()
}

/// (depth, kind) of each node, in preorder.
fn shape(l: &BlockLayout) -> Vec<(usize, FragmentKind)> {
    let tree = l.fragment_tree().unwrap();
    tree.nodes().iter().map(|n| (n.depth(), n.kind())).collect()
}

/// Every fragment once, parents before children, subtrees nested.
fn check_invariants(l: &BlockLayout) {
    let tree = l.fragment_tree().unwrap();
    let nodes = tree.nodes();
    let total = l.fragments().len() + l.lines().len() + l.inlines().len() + l.texts().len();
    assert_eq!(nodes.len(), total);
    let kinds: HashSet<FragmentKind> = nodes.iter().map(|n| n.kind()).collect();
    assert_eq!(kinds.len(), total);
    for (i, n) in nodes.iter().enumerate() {
        assert!(n.subtree_end() > i && n.subtree_end() <= nodes.len());
        if let Some(p) = n.parent() {
            assert!(p < i && nodes[p].subtree_end() >= n.subtree_end());
            assert_eq!(n.depth(), nodes[p].depth() + 1);
            assert!(tree.children(p).any(|c| c == i));
        } else {
            assert_eq!(n.depth(), 0);
        }
        // Texts are leaves; lines hang off blocks.
        if let FragmentKind::Text(_) = n.kind() {
            assert_eq!(n.subtree_end(), i + 1);
        }
        if let FragmentKind::Line(_) = n.kind() {
            assert!(matches!(
                nodes[n.parent().unwrap()].kind(),
                FragmentKind::Block(_)
            ));
        }
    }
}

#[test]
fn blocks_lines_inline_boxes_and_text_nest_in_document_order() {
    use FragmentKind::{Block, Inline, Line, Text};
    let l = layout("<section>x<span>xx<div>xxx</div>xx</span>x</section>", "");
    assert_eq!(
        shape(&l),
        [
            (0, Block(0)),
            (1, Block(1)),
            (2, Block(2)),
            (3, Line(0)),
            (4, Text(0)),
            (4, Inline(0)),
            (5, Text(1)),
            (3, Block(3)),
            (4, Line(1)),
            (5, Text(2)),
            (3, Line(2)),
            (4, Inline(1)),
            (5, Text(3)),
            (4, Text(4)),
        ]
    );
    check_invariants(&l);
    let tree = l.fragment_tree().unwrap();
    let dump = tree.dump(&l, 1 << 16).unwrap();
    assert!(
        dump.starts_with("block 0 (0,0,100,30)\n  block 1 (0,0,100,30)\n"),
        "{dump}"
    );
    assert!(dump.contains("\n        line 1 (0,10,100,10)\n          text 2 utf16=0..3 "));
    assert_eq!(tree.dump(&l, dump.len()).unwrap(), dump);
    assert_eq!(tree.dump(&l, dump.len() - 1), Err(FlowError::OutputLimit));
    let children: Vec<usize> = tree.children(2).collect();
    assert_eq!(children, [3, 7, 10]);
}

#[test]
fn atomic_inlines_sit_inside_their_inline_box_on_the_line() {
    use FragmentKind::{Block, Inline, Line, Text};
    let l = layout(
        "<p>x<b><span class=ib>xx</span></b>x",
        "b { font-weight:400 }",
    );
    assert_eq!(
        shape(&l)[2..],
        [
            (2, Block(2)),
            (3, Line(0)),
            (4, Text(0)),
            (4, Inline(0)),
            (5, Block(3)),
            (6, Line(1)),
            // The paragraph's own texts come first in the text list.
            (7, Text(2)),
            (4, Text(1)),
        ]
    );
    check_invariants(&l);
}

#[test]
fn floats_and_positioned_boxes_stay_under_their_parent_block() {
    let l = layout(
        "<main><div id=f></div>xx<div id=a></div><p>x</p></main>",
        "#f { float:left; width:10px; height:10px } \
         #a { position:absolute; top:0; height:5px } main { position:relative }",
    );
    check_invariants(&l);
    let tree = l.fragment_tree().unwrap();
    let main = tree
        .nodes()
        .iter()
        .position(|n| n.kind() == FragmentKind::Block(2))
        .unwrap();
    let kinds: Vec<FragmentKind> = tree
        .children(main)
        .map(|c| tree.nodes()[c].kind())
        .collect();
    assert_eq!(
        kinds,
        [
            FragmentKind::Block(3),
            FragmentKind::Line(0),
            FragmentKind::Block(4),
            FragmentKind::Block(5)
        ]
    );
}

#[test]
fn deep_trees_are_built_without_recursion() {
    let depth = 200;
    let html = format!("{}x{}", "<div>".repeat(depth), "</div>".repeat(depth));
    let l = layout(&html, "");
    check_invariants(&l);
    let tree = l.fragment_tree().unwrap();
    let deepest = tree.nodes().iter().map(|n| n.depth()).max().unwrap();
    // html, body, the divs, the line and the text.
    assert_eq!(deepest, depth + 3);
}

#[test]
fn block_only_layouts_have_block_only_trees() {
    let (doc, styles) = common::page("<main><div></div><div></div></main>", "* { margin:0 }");
    let tree = BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    let l = BlockLayout::compute(
        &tree,
        CssSize::new(100.0, 100.0).unwrap(),
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    check_invariants(&l);
    let depths: Vec<usize> = shape(&l).iter().map(|s| s.0).collect();
    assert_eq!(depths, [0, 1, 2, 3, 3]);
}
