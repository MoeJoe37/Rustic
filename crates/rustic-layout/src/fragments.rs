//! The general fragment tree: block, line, inline-box and text fragments of
//! one layout in a single preorder tree with checked geometry.
//!
//! [`crate::BlockLayout`] keeps each fragment kind in its own list, which
//! is convenient for painting each kind. Consumers that walk the result as
//! a tree (hit testing, accessibility, dumps, incremental layout) need the
//! containment between kinds too: lines inside their block, inline boxes
//! and text inside their line (nested by inline box), and atomic inlines
//! inside the inline box or line that holds them. Children are in document
//! order. The tree is built iteratively, so deep content cannot exhaust the
//! stack.

use crate::flow::FlowError;
use crate::{BlockLayout, CssRect};

/// What a [`FragmentNode`] refers to: an index into the matching
/// [`BlockLayout`] list.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FragmentKind {
    /// [`BlockLayout::fragments`] (block containers, replaced boxes and
    /// atomic inlines).
    Block(usize),
    /// [`BlockLayout::lines`].
    Line(usize),
    /// [`BlockLayout::inlines`].
    Inline(usize),
    /// [`BlockLayout::texts`].
    Text(usize),
}

/// One node of a [`FragmentTree`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FragmentNode {
    kind: FragmentKind,
    parent: Option<usize>,
    end: usize,
    depth: usize,
    rect: CssRect,
}

impl FragmentNode {
    /// The fragment this node stands for.
    pub const fn kind(self) -> FragmentKind {
        self.kind
    }
    /// Parent node index; `None` for the root element's block. Floats and
    /// positioned boxes sit under their parent block in the box tree (their
    /// containing block is [`crate::BlockFragment::containing_block`]).
    pub const fn parent(self) -> Option<usize> {
        self.parent
    }
    /// End (exclusive) of this node's subtree in [`FragmentTree::nodes`].
    pub const fn subtree_end(self) -> usize {
        self.end
    }
    /// Distance from the node's root.
    pub const fn depth(self) -> usize {
        self.depth
    }
    /// The fragment's border box (blocks, inline boxes), line box or text
    /// content area.
    pub const fn rect(self) -> CssRect {
        self.rect
    }
}

/// Every fragment of a layout in one preorder tree.
#[derive(Clone, Debug, PartialEq)]
pub struct FragmentTree {
    nodes: Vec<FragmentNode>,
}

impl FragmentTree {
    /// Nodes in preorder; a node's subtree is `index..subtree_end()`.
    pub fn nodes(&self) -> &[FragmentNode] {
        &self.nodes
    }

    /// Child node indexes of node `i`, in document order.
    pub fn children(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let end = self.nodes.get(i).map_or(i, |n| n.end);
        let mut next = i + 1;
        std::iter::from_fn(move || {
            if next >= end {
                return None;
            }
            let child = next;
            next = self.nodes[child].end;
            Some(child)
        })
    }

    /// Deterministic indented dump, with no partial output on overflow.
    pub fn dump(&self, layout: &BlockLayout, max_bytes: usize) -> Result<String, FlowError> {
        let mut out = String::new();
        for n in &self.nodes {
            let r = n.rect;
            let label = match n.kind {
                FragmentKind::Block(i) => format!("block {i}"),
                FragmentKind::Line(i) => format!("line {i}"),
                FragmentKind::Inline(i) => format!("inline {i}"),
                FragmentKind::Text(i) => {
                    let t = layout.texts().get(i).ok_or(FlowError::InvalidSnapshot)?;
                    let range = t.text();
                    format!("text {i} utf16={}..{}", range.start, range.end)
                }
            };
            let line = format!(
                "{}{label} ({},{},{},{})\n",
                "  ".repeat(n.depth.min(64)),
                r.origin().x.get(),
                r.origin().y.get(),
                r.size().width().get(),
                r.size().height().get()
            );
            if line.len() > max_bytes.saturating_sub(out.len()) {
                return Err(FlowError::OutputLimit);
            }
            out.push_str(&line);
        }
        Ok(out)
    }
}

/// A child entry before ordering: (document-order key, tie-break, kind).
type Entry = (usize, usize, FragmentKind);

impl BlockLayout {
    /// Build the general fragment tree of this layout.
    ///
    /// Block children are ordered by source position; lines of anonymous
    /// blocks are placed where their anonymous block was, between block
    /// siblings. Texts and inline boxes nest by inline box on each line.
    pub fn fragment_tree(&self) -> Result<FragmentTree, FlowError> {
        let fragments = self.fragments();
        let (nb, nl, ni, nt) = (
            fragments.len(),
            self.lines().len(),
            self.inlines().len(),
            self.texts().len(),
        );
        // Children lists per parent kind.
        let mut of_block: Vec<Vec<Entry>> = vec![Vec::new(); nb];
        let mut of_line: Vec<Vec<Entry>> = vec![Vec::new(); nl];
        let mut of_inline: Vec<Vec<Entry>> = vec![Vec::new(); ni];
        let mut roots: Vec<Entry> = Vec::new();
        let order = |i: usize| self.fragment_order.get(i).copied().unwrap_or(i);
        for (i, f) in fragments.iter().enumerate() {
            let entry = (order(i), 1, FragmentKind::Block(i));
            match (f.line(), f.inline_parent, f.parent()) {
                (Some(_), Some(p), _) if p < ni => of_inline[p].push(entry),
                (Some(l), _, _) if l < nl => of_line[l].push(entry),
                (_, _, Some(p)) if p < nb => of_block[p].push(entry),
                (_, _, None) => roots.push(entry),
                _ => return Err(FlowError::InvalidSnapshot),
            }
        }
        for (i, l) in self.lines().iter().enumerate() {
            let key = self.line_order.get(i).copied().unwrap_or(0);
            of_block
                .get_mut(l.block())
                .ok_or(FlowError::InvalidSnapshot)?
                .push((key, 0, FragmentKind::Line(i)));
        }
        // Inline boxes and texts in visual order within their parent.
        for (i, f) in self.inlines().iter().enumerate() {
            let entry = (0, i, FragmentKind::Inline(i));
            match f.parent() {
                Some(p) if p < ni => of_inline[p].push(entry),
                None if f.line() < nl => of_line[f.line()].push(entry),
                _ => return Err(FlowError::InvalidSnapshot),
            }
        }
        for (i, t) in self.texts().iter().enumerate() {
            let entry = (0, ni + i, FragmentKind::Text(i));
            match t.parent() {
                Some(p) if p < ni => of_inline[p].push(entry),
                None if t.line() < nl => of_line[t.line()].push(entry),
                _ => return Err(FlowError::InvalidSnapshot),
            }
        }
        // Inline-level children sort by their left edge (visual order on
        // the line); blocks and lines by document order.
        let left = |k: FragmentKind| -> f64 {
            match k {
                FragmentKind::Block(i) => fragments[i].border().origin().x.get(),
                FragmentKind::Inline(i) => self.inlines()[i].border().origin().x.get(),
                FragmentKind::Text(i) => self.texts()[i].rect().origin().x.get(),
                FragmentKind::Line(_) => 0.0,
            }
        };
        let by_document = |list: &mut Vec<Entry>| list.sort_by_key(|e| (e.0, e.1));
        let by_visual = |list: &mut Vec<Entry>| {
            list.sort_by(|a, b| left(a.2).total_cmp(&left(b.2)).then(a.1.cmp(&b.1)));
        };
        of_block.iter_mut().for_each(by_document);
        by_document(&mut roots);
        of_line.iter_mut().for_each(by_visual);
        of_inline.iter_mut().for_each(by_visual);

        let total = nb + nl + ni + nt;
        let mut nodes: Vec<FragmentNode> = Vec::with_capacity(total);
        let rect = |k: FragmentKind| -> CssRect {
            match k {
                FragmentKind::Block(i) => fragments[i].border(),
                FragmentKind::Line(i) => self.lines()[i].rect(),
                FragmentKind::Inline(i) => self.inlines()[i].border(),
                FragmentKind::Text(i) => self.texts()[i].rect(),
            }
        };
        // Explicit stack: (node index, remaining children reversed).
        let children = |k: FragmentKind| -> &Vec<Entry> {
            match k {
                FragmentKind::Block(i) => &of_block[i],
                FragmentKind::Line(i) => &of_line[i],
                FragmentKind::Inline(i) => &of_inline[i],
                FragmentKind::Text(_) => &roots, // never read: texts are leaves
            }
        };
        let mut stack: Vec<(usize, usize)> = Vec::new();
        let mut root_next = 0;
        loop {
            let next = match stack.last_mut() {
                Some((parent, cursor)) => {
                    let list = match nodes[*parent].kind {
                        FragmentKind::Text(_) => None,
                        k => Some(children(k)),
                    };
                    match list.and_then(|l| l.get(*cursor)) {
                        Some(e) => {
                            *cursor += 1;
                            Some((Some(*parent), e.2))
                        }
                        None => {
                            let (done, _) = stack.pop().ok_or(FlowError::InvalidSnapshot)?;
                            nodes[done].end = nodes.len();
                            continue;
                        }
                    }
                }
                None => match roots.get(root_next) {
                    Some(e) => {
                        root_next += 1;
                        Some((None, e.2))
                    }
                    None => None,
                },
            };
            let Some((parent, kind)) = next else {
                break;
            };
            if nodes.len() >= total {
                // A cycle or a fragment reached twice.
                return Err(FlowError::InvalidSnapshot);
            }
            let depth = parent.map_or(0, |p| nodes[p].depth + 1);
            nodes.push(FragmentNode {
                kind,
                parent,
                end: nodes.len() + 1,
                depth,
                rect: rect(kind),
            });
            stack.push((nodes.len() - 1, 0));
        }
        if nodes.len() != total {
            return Err(FlowError::InvalidSnapshot);
        }
        Ok(FragmentTree { nodes })
    }
}
