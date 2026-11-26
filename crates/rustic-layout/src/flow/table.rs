//! Table layout (CSS 2.2 §17, CSS Tables 3 Editor's Draft reviewed
//! 2026-11-27) as a formatting context inside the block walk.
//!
//! A table's frame drives a small state machine, like grid layout. Its rows
//! are gathered in display order (header groups first, footer groups last)
//! and cells are given slots with the HTML table processing model's
//! `colspan`/`rowspan` rules (a `rowspan` stops at the end of its row
//! group, `0` spans to it). Columns are sized with the automatic algorithm
//! from the cells' min/max-content contributions (spanning cells widen the
//! columns they cross in proportion to their maximum widths), or with the
//! fixed algorithm from the first row when `table-layout: fixed` has a
//! definite width. Captions and cells are then laid out at their widths;
//! rows take the tallest cell (with baseline alignment), spanning cells
//! grow the rows they span, a taller specified table height is shared by the
//! rows, and `vertical-align` moves a cell's content inside its slot.
//!
//! [`fixup`] generates anonymous table, row and cell boxes while flow nodes
//! are built. The separated borders model only: `border-collapse: collapse`
//! uses no spacing and keeps every border; column widths, percentages,
//! `visibility: collapse` and `empty-cells` are later steps (TD-211).

use rustic_css::{
    BorderCollapse, CaptionSide, Direction, LengthPercentage, PropertyId as P, PropertyValue as V,
    TableLayout, VerticalAlign, VerticalAlignKeyword,
};
use rustic_style::ComputedStyle;

use super::{Bfc, Engine, FlowError, Frame, Item, Work};
use crate::height::HeightConstraint;
use crate::intrinsic::{self, Intrinsic, Resolver, Sizing};
use crate::sizing::{box_edges, lp, measure_in_band};
use crate::{BlockMeasure, CssPx, Edges};
use crate::{BoxKind, BoxSnapshot};

const EMPTY: BlockMeasure = BlockMeasure {
    content_width: CssPx::ZERO,
    margin: NO_EDGES,
    padding: NO_EDGES,
    border: NO_EDGES,
};
const NO_EDGES: Edges = Edges {
    top: CssPx::ZERO,
    right: CssPx::ZERO,
    bottom: CssPx::ZERO,
    left: CssPx::ZERO,
};

/// Columns past this are dropped with their cells' remaining span (the
/// HTML model caps `colspan` at 1000 but not the column count).
const MAX_COLUMNS: usize = 10_000;

/// The table role of a flow node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TablePart {
    Table,
    Caption,
    HeaderGroup,
    BodyGroup,
    FooterGroup,
    Row,
    Cell,
    /// A column or column group: no layout of its own yet.
    Column,
}

impl TablePart {
    pub(super) fn of(display: rustic_css::Display) -> Option<Self> {
        use rustic_css::Display as D;
        Some(match display {
            D::Table | D::InlineTable => Self::Table,
            D::TableCaption => Self::Caption,
            D::TableHeaderGroup => Self::HeaderGroup,
            D::TableRowGroup => Self::BodyGroup,
            D::TableFooterGroup => Self::FooterGroup,
            D::TableRow => Self::Row,
            D::TableCell => Self::Cell,
            D::TableColumn | D::TableColumnGroup => Self::Column,
            _ => return None,
        })
    }

    fn group(self) -> bool {
        matches!(
            self,
            Self::HeaderGroup | Self::BodyGroup | Self::FooterGroup
        )
    }

    /// Whether a box of this role may be a child of `parent` without
    /// anonymous table boxes (CSS Tables 3 §3.9, not implemented yet).
    pub(super) fn fits_in(self, parent: Option<Self>) -> bool {
        match self {
            Self::Table => !matches!(
                parent,
                Some(Self::Table | Self::Row)
                    | Some(Self::HeaderGroup)
                    | Some(Self::BodyGroup)
                    | Some(Self::FooterGroup)
                    | Some(Self::Column)
            ),
            Self::Caption => parent == Some(Self::Table),
            Self::HeaderGroup | Self::BodyGroup | Self::FooterGroup => parent == Some(Self::Table),
            Self::Row => parent.is_some_and(|p| p == Self::Table || p.group()),
            Self::Cell => parent == Some(Self::Row),
            Self::Column => matches!(parent, Some(Self::Table | Self::Column)),
        }
    }

    /// Whether this role's children must all be table parts (whitespace
    /// aside): tables, row groups, rows and column groups.
    pub(super) fn needs_proper_children(self) -> bool {
        matches!(
            self,
            Self::Table
                | Self::HeaderGroup
                | Self::BodyGroup
                | Self::FooterGroup
                | Self::Row
                | Self::Column
        )
    }
}

/// One child after table fixup: a flattened item, or an anonymous table
/// box around items (fixed up again when it is built).
#[derive(Debug)]
pub(super) enum Out {
    Item(Item),
    Wrap(TablePart, Vec<Item>),
}

fn part_of(boxes: &[BoxSnapshot], item: Item) -> Option<TablePart> {
    match item {
        Item::Block(c) => match boxes[c].kind() {
            BoxKind::Element(d) => TablePart::of(d),
            BoxKind::Text => None,
        },
        _ => None,
    }
}

/// Generate anonymous table boxes among the flattened children `items` of
/// a box with table role `parent` (CSS Tables 3 §3.9.1): in a table, row
/// group or row, runs of other content are wrapped in an anonymous row
/// (cells in a table or group) or cell (other content in a row), and
/// whitespace-only runs are dropped; columns keep only columns. Elsewhere,
/// runs of table parts that are misparented (with the whitespace between
/// them) are wrapped in an anonymous table. Wrappers are fixed up again
/// when built, so a misparented cell becomes table, row and cell.
pub(super) fn fixup(
    parent: Option<TablePart>,
    boxes: &[BoxSnapshot],
    items: Vec<Item>,
    work: &mut Work<'_>,
) -> Result<Vec<Out>, FlowError> {
    let mut out = Vec::with_capacity(items.len());
    let mut run: Vec<Item> = Vec::new();
    let mut spaces: Vec<Item> = Vec::new();
    match parent.filter(|p| p.needs_proper_children()) {
        Some(p) => {
            let wrapper = match p {
                TablePart::Row => Some(TablePart::Cell),
                TablePart::Column => None,
                _ => Some(TablePart::Row),
            };
            let flush = |run: &mut Vec<Item>, out: &mut Vec<Out>| {
                let content = run.iter().any(|i| !matches!(i, Item::Whitespace(_)));
                let taken = std::mem::take(run);
                if let (true, Some(w)) = (content, wrapper) {
                    out.push(Out::Wrap(w, taken));
                }
            };
            for item in items {
                work.charge(1)?;
                if part_of(boxes, item).is_some_and(|c| c.fits_in(Some(p))) {
                    flush(&mut run, &mut out);
                    out.push(Out::Item(item));
                } else {
                    run.push(item);
                }
            }
            flush(&mut run, &mut out);
        }
        None => {
            for item in items {
                work.charge(1)?;
                let misparented = part_of(boxes, item).is_some_and(|c| !c.fits_in(parent));
                if misparented {
                    run.append(&mut spaces);
                    run.push(item);
                } else if matches!(item, Item::Whitespace(_)) && !run.is_empty() {
                    spaces.push(item);
                } else {
                    if !run.is_empty() {
                        out.push(Out::Wrap(TablePart::Table, std::mem::take(&mut run)));
                    }
                    out.extend(spaces.drain(..).map(Out::Item));
                    out.push(Out::Item(item));
                }
            }
            if !run.is_empty() {
                out.push(Out::Wrap(TablePart::Table, run));
            }
            out.extend(spaces.into_iter().map(Out::Item));
        }
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Align {
    Baseline,
    Top,
    Middle,
    Bottom,
}

fn cell_align(style: &ComputedStyle) -> Align {
    match style.get(P::VerticalAlign) {
        V::VerticalAlign(VerticalAlign::Keyword(k)) => match k {
            VerticalAlignKeyword::Top => Align::Top,
            VerticalAlignKeyword::Middle => Align::Middle,
            VerticalAlignKeyword::Bottom => Align::Bottom,
            // `sub`, `super`, `text-top`, `text-bottom` and lengths act as
            // `baseline` in cells (CSS 2.2 §17.5.3).
            _ => Align::Baseline,
        },
        _ => Align::Baseline,
    }
}

#[derive(Clone, Copy, Debug)]
struct Cell {
    node: usize,
    row: usize,
    column: usize,
    rows: usize,
    columns: usize,
    align: Align,
    /// Outer (border-box) min/max-content contributions.
    min: f64,
    max: f64,
    measure: BlockMeasure,
    /// Content height from the latest layout.
    natural: f64,
    baseline: Option<f64>,
}

impl Cell {
    fn horizontal_extras(&self) -> f64 {
        let m = &self.measure;
        m.padding.left.get() + m.padding.right.get() + m.border.left.get() + m.border.right.get()
    }
    fn vertical_extras(&self) -> f64 {
        let m = &self.measure;
        m.padding.top.get() + m.padding.bottom.get() + m.border.top.get() + m.border.bottom.get()
    }
}

#[derive(Clone, Copy, Debug)]
struct Caption {
    node: usize,
    bottom: bool,
    measure: BlockMeasure,
    natural: f64,
}

#[derive(Clone, Copy, Debug)]
struct Group {
    /// `None` for rows directly in the table.
    node: Option<usize>,
    rows: (usize, usize),
}

#[derive(Clone, Copy, Debug)]
struct Row {
    node: usize,
    /// Specified height (a minimum).
    minimum: f64,
}

#[derive(Clone, Copy, Debug)]
enum Target {
    Cell(usize),
    Caption(usize),
}

/// The table's rows, groups, cells in slots and captions.
struct Structure {
    groups: Vec<Group>,
    rows: Vec<Row>,
    cells: Vec<Cell>,
    captions: Vec<Caption>,
    columns: usize,
}

/// One table's layout state.
pub(super) struct TableRun {
    table: usize,
    direction: Direction,
    /// Content-box width.
    width: f64,
    spacing: (f64, f64),
    structure: Structure,
    /// Column left edges (physical, in the content box).
    lefts: Vec<f64>,
    queue: Vec<Target>,
    current: Option<Target>,
}

/// `border-spacing` in pixels; zero in the collapsing model.
fn spacing(style: &ComputedStyle) -> Result<(f64, f64), FlowError> {
    if matches!(
        style.get(P::BorderCollapse),
        V::BorderCollapse(BorderCollapse::Collapse)
    ) {
        return Ok((0.0, 0.0));
    }
    match style.get(P::BorderSpacing) {
        V::BorderSpacing(h, v) => Ok((lp(h, 0.0)?.max(0.0), lp(v, 0.0)?.max(0.0))),
        _ => Err(FlowError::InvalidSnapshot),
    }
}

/// `table-layout: fixed` with a width that is not `auto` (CSS 2.2 §17.5.2).
fn fixed_layout(style: &ComputedStyle) -> Result<bool, FlowError> {
    Ok(matches!(
        style.get(P::TableLayout),
        V::TableLayout(TableLayout::Fixed)
    ) && !matches!(Sizing::read(style, P::Width)?, Sizing::Initial))
}

/// Total spacing around `count` columns or rows.
fn gaps(count: usize, space: f64) -> f64 {
    if count == 0 {
        0.0
    } else {
        space * (count as f64 + 1.0)
    }
}

/// Add `extra` over `targets` in proportion to `weights` (evenly when the
/// weights are all zero).
fn share(targets: &mut [f64], weights: &[f64], extra: f64) {
    if targets.is_empty() || extra <= 0.0 {
        return;
    }
    let total: f64 = weights.iter().sum();
    let n = targets.len() as f64;
    for (t, w) in targets.iter_mut().zip(weights) {
        *t += if total > 0.0 {
            extra * w / total
        } else {
            extra / n
        };
    }
}

/// Column minimum and maximum widths from the cells (CSS 2.2 §17.5.2.2):
/// single-column cells first, then spanning cells by increasing span.
fn column_bounds(cells: &[Cell], columns: usize, space: f64) -> (Vec<f64>, Vec<f64>) {
    let mut min = vec![0.0_f64; columns];
    let mut max = vec![0.0_f64; columns];
    let mut spanning: Vec<&Cell> = Vec::new();
    for c in cells {
        if c.columns == 1 {
            min[c.column] = min[c.column].max(c.min);
            max[c.column] = max[c.column].max(c.max);
        } else {
            spanning.push(c);
        }
    }
    spanning.sort_by_key(|c| c.columns);
    for c in spanning {
        let range = c.column..c.column + c.columns;
        let inner = space * (c.columns as f64 - 1.0);
        let weights: Vec<f64> = max[range.clone()].to_vec();
        let have: f64 = min[range.clone()].iter().sum::<f64>() + inner;
        share(&mut min[range.clone()], &weights, c.min - have);
        let have: f64 = max[range.clone()].iter().sum::<f64>() + inner;
        let weights: Vec<f64> = max[range.clone()].to_vec();
        share(&mut max[range.clone()], &weights, c.max - have);
        for i in range {
            max[i] = max[i].max(min[i]);
        }
    }
    (min, max)
}

/// Automatic column widths for `available` space between the spacing.
fn auto_widths(min: &[f64], max: &[f64], available: f64) -> Vec<f64> {
    let (sum_min, sum_max): (f64, f64) = (min.iter().sum(), max.iter().sum());
    if available <= sum_min {
        return min.to_vec();
    }
    if available <= sum_max {
        let t = if sum_max > sum_min {
            (available - sum_min) / (sum_max - sum_min)
        } else {
            0.0
        };
        return min.iter().zip(max).map(|(a, b)| a + (b - a) * t).collect();
    }
    let mut widths = max.to_vec();
    share(&mut widths, max, available - sum_max);
    widths
}

impl Engine<'_, '_> {
    /// Gather the table's rows, cells and captions and place cells in slots.
    fn table_structure(&mut self, t: usize) -> Result<Structure, FlowError> {
        let end = self.nodes[t].end;
        let mut captions = Vec::new();
        // Groups in source order; direct rows join one implicit group per run.
        let mut source: Vec<(TablePart, Option<usize>, Vec<usize>)> = Vec::new();
        let mut c = t + 1;
        while c < end {
            self.work.charge(1)?;
            let next = self.nodes[c].end;
            match self.nodes[c].table {
                Some(TablePart::Caption) => {
                    let bottom = matches!(
                        self.nodes[c].style.get(P::CaptionSide),
                        V::CaptionSide(CaptionSide::Bottom)
                    );
                    captions.push(Caption {
                        node: c,
                        bottom,
                        measure: EMPTY,
                        natural: 0.0,
                    });
                }
                Some(TablePart::Row) => match source.last_mut() {
                    Some((TablePart::Row, None, rows)) => rows.push(c),
                    _ => source.push((TablePart::Row, None, vec![c])),
                },
                Some(part) if part.group() => {
                    let mut rows = Vec::new();
                    let mut r = c + 1;
                    while r < next {
                        self.work.charge(1)?;
                        if self.nodes[r].table == Some(TablePart::Row) {
                            rows.push(r);
                        }
                        r = self.nodes[r].end;
                    }
                    source.push((part, Some(c), rows));
                }
                _ => {}
            }
            c = next;
        }
        // Header groups display first and footer groups last (CSS 2.2 §17.2).
        let rank = |p: TablePart| match p {
            TablePart::HeaderGroup => 0,
            TablePart::FooterGroup => 2,
            _ => 1,
        };
        source.sort_by_key(|(p, _, _)| rank(*p));
        let mut groups = Vec::with_capacity(source.len());
        let mut rows = Vec::new();
        let mut cells = Vec::new();
        let mut busy: Vec<usize> = Vec::new();
        for (_, node, list) in source {
            let first = rows.len();
            let last = first + list.len();
            for (offset, &r) in list.iter().enumerate() {
                let index = first + offset;
                let style = self.nodes[r].style;
                let minimum = HeightConstraint::new(style, None, CssPx::ZERO)?
                    .specified()
                    .map_or(0.0, CssPx::get);
                rows.push(Row { node: r, minimum });
                let mut column = 0;
                let mut k = r + 1;
                while k < self.nodes[r].end {
                    self.work.charge(1)?;
                    let next = self.nodes[k].end;
                    if self.nodes[k].table != Some(TablePart::Cell) {
                        k = next;
                        continue;
                    }
                    while busy.get(column).is_some_and(|&until| until > index) {
                        column += 1;
                    }
                    if column >= MAX_COLUMNS {
                        k = next;
                        continue;
                    }
                    let (span_columns, span_rows) = self.nodes[k].span;
                    let span_columns = (span_columns as usize).clamp(1, MAX_COLUMNS - column);
                    let span_rows = match span_rows as usize {
                        0 => last - index,
                        n => n.min(last - index),
                    }
                    .max(1);
                    if busy.len() < column + span_columns {
                        busy.resize(column + span_columns, 0);
                    }
                    self.work.charge(span_columns)?;
                    for b in &mut busy[column..column + span_columns] {
                        *b = (*b).max(index + span_rows);
                    }
                    let n = &self.nodes[k];
                    let outer = intrinsic::outer_contribution(n.style, n.intrinsic)?;
                    let own_min = n.intrinsic.min + intrinsic::fixed_extras(n.style)?;
                    let min = outer.min.max(own_min);
                    cells.push(Cell {
                        node: k,
                        row: index,
                        column,
                        rows: span_rows,
                        columns: span_columns,
                        align: cell_align(n.style),
                        min,
                        max: outer.max.max(min),
                        measure: EMPTY,
                        natural: 0.0,
                        baseline: None,
                    });
                    column += span_columns;
                    k = next;
                }
            }
            // Rows spanning into the next group are cut at its start.
            for b in &mut busy {
                *b = (*b).min(last);
            }
            groups.push(Group {
                node,
                rows: (first, last),
            });
        }
        Ok(Structure {
            groups,
            rows,
            cells,
            captions,
            columns: busy.len(),
        })
    }

    /// The table's content-box min/max-content widths.
    pub(super) fn table_intrinsic(&mut self, t: usize) -> Result<Intrinsic, FlowError> {
        let space = spacing(self.nodes[t].style)?.0;
        let s = self.table_structure(t)?;
        let mut result = if fixed_layout(self.nodes[t].style)? {
            // Only the first row's fixed widths count (§17.5.2.1).
            let widths = self.column_widths(t, &s, 0.0, space)?;
            let w = widths.iter().sum::<f64>() + gaps(s.columns, space);
            Intrinsic { min: w, max: w }
        } else {
            let (min, max) = column_bounds(&s.cells, s.columns, space);
            Intrinsic {
                min: min.iter().sum::<f64>() + gaps(s.columns, space),
                max: max.iter().sum::<f64>() + gaps(s.columns, space),
            }
        };
        for caption in &s.captions {
            let n = &self.nodes[caption.node];
            let outer = intrinsic::outer_contribution(n.style, n.intrinsic)?;
            result.min = result.min.max(outer.min);
        }
        result.max = result.max.max(result.min);
        Ok(result)
    }

    /// The used content-box width of a block-level table in `available`
    /// space (after its margins, padding and borders): its specified width
    /// or shrink-to-fit, never below its minimum width (CSS 2.2 §17.5.2).
    pub(super) fn table_width(&self, t: usize, containing: f64) -> Result<f64, FlowError> {
        let n = &self.nodes[t];
        let style = n.style;
        let (padding, border) = box_edges(style, containing)?;
        let extras =
            padding.left.get() + padding.right.get() + border.left.get() + border.right.get();
        let margin = |p| -> Result<f64, FlowError> {
            Ok(crate::sizing::auto(style, p, containing)?.unwrap_or(0.0))
        };
        let available = containing - margin(P::MarginLeft)? - margin(P::MarginRight)? - extras;
        let resolver = Resolver::new(style, Some(containing), extras, n.intrinsic);
        let (specified, minimum, maximum) = resolver.widths(available)?;
        let tentative = specified.unwrap_or_else(|| n.intrinsic.fit(available));
        Ok(maximum
            .map_or(tentative, |m| tentative.min(m))
            .max(minimum)
            .max(n.intrinsic.min)
            .max(0.0))
    }

    /// Column widths for content width `width` (CSS 2.2 §17.5.2).
    fn column_widths(
        &mut self,
        t: usize,
        s: &Structure,
        width: f64,
        space: f64,
    ) -> Result<Vec<f64>, FlowError> {
        let available = (width - gaps(s.columns, space)).max(0.0);
        if !fixed_layout(self.nodes[t].style)? {
            let (min, max) = column_bounds(&s.cells, s.columns, space);
            return Ok(auto_widths(&min, &max, available));
        }
        // Fixed layout: the first row's cells with a fixed width set their
        // columns; the other columns share what is left (§17.5.2.1).
        let mut widths: Vec<Option<f64>> = vec![None; s.columns];
        for c in s.cells.iter().filter(|c| c.row == 0) {
            self.work.charge(1)?;
            let n = &self.nodes[c.node];
            let w = match Sizing::read(n.style, P::Width)? {
                Sizing::Length(v) if !intrinsic::has_percentage(v) => {
                    let extras = intrinsic::fixed_extras(n.style)?;
                    let r = Resolver::new(n.style, None, extras, n.intrinsic);
                    r.resolve(Sizing::Length(v), 0.0)?.map(|w| w + extras)
                }
                Sizing::Length(LengthPercentage::Percentage(p)) => Some(available * p / 100.0),
                _ => None,
            };
            if let Some(w) = w {
                let each = (w - space * (c.columns as f64 - 1.0)).max(0.0) / c.columns as f64;
                for slot in &mut widths[c.column..c.column + c.columns] {
                    *slot = Some(each);
                }
            }
        }
        let used: f64 = widths.iter().flatten().sum();
        let free = (available - used).max(0.0);
        let auto = widths.iter().filter(|w| w.is_none()).count();
        let mut out: Vec<f64> = widths
            .iter()
            .map(|w| w.unwrap_or(if auto > 0 { free / auto as f64 } else { 0.0 }))
            .collect();
        if auto == 0 {
            let weights = out.clone();
            share(&mut out, &weights, free);
        }
        Ok(out)
    }

    /// Start laying out table `t`, whose width is already set. Returns its
    /// index in `self.tables`.
    pub(super) fn start_table(&mut self, t: usize) -> Result<usize, FlowError> {
        let width = self.nodes[t].measure.content_width.get();
        let direction = self.nodes[t].direction;
        let space = spacing(self.nodes[t].style)?;
        let mut s = self.table_structure(t)?;
        let widths = self.column_widths(t, &s, width, space.0)?;
        // Left edges from the inline start, mirrored in RTL.
        let mut lefts = Vec::with_capacity(widths.len());
        let mut x = space.0;
        for &w in &widths {
            lefts.push(match direction {
                Direction::Ltr => x,
                Direction::Rtl => width - x - w,
            });
            x += w + space.0;
        }
        let mut queue = Vec::with_capacity(s.cells.len() + s.captions.len());
        for (i, caption) in s.captions.iter_mut().enumerate() {
            let n = &self.nodes[caption.node];
            caption.measure = measure_in_band(
                n.style,
                CssPx::new(width)?,
                direction,
                (0.0, 0.0),
                Some(n.intrinsic),
                None,
            )?;
            queue.push(Target::Caption(i));
        }
        for (i, cell) in s.cells.iter_mut().enumerate() {
            let n = &self.nodes[cell.node];
            let (padding, border) = box_edges(n.style, width)?;
            cell.measure = BlockMeasure {
                content_width: CssPx::ZERO,
                margin: Edges::default(),
                padding,
                border,
            };
            let span = widths[cell.column..cell.column + cell.columns]
                .iter()
                .sum::<f64>()
                + space.0 * (cell.columns as f64 - 1.0);
            cell.measure.content_width = CssPx::new((span - cell.horizontal_extras()).max(0.0))?;
            queue.push(Target::Cell(i));
        }
        queue.reverse();
        self.tables.push(TableRun {
            table: t,
            direction,
            width,
            spacing: space,
            structure: s,
            lefts,
            queue,
            current: None,
        });
        Ok(self.tables.len() - 1)
    }

    /// Advance table `k`: push the next cell or caption frame and return
    /// `None`, or finish and return the table's natural content height.
    pub(super) fn table_step(
        &mut self,
        frames: &mut Vec<Frame>,
        bfcs: &mut Vec<Bfc>,
        k: usize,
    ) -> Result<Option<f64>, FlowError> {
        self.work.charge(1)?;
        if let Some(target) = self.tables[k].queue.pop() {
            let s = &self.tables[k].structure;
            let (node, measure) = match target {
                Target::Cell(i) => (s.cells[i].node, s.cells[i].measure),
                Target::Caption(i) => (s.captions[i].node, s.captions[i].measure),
            };
            let n = &mut self.nodes[node];
            n.measure = measure;
            n.height = Some(HeightConstraint::solved(None));
            n.top_resolved = true;
            self.tables[k].current = Some(target);
            frames.push(self.root_frame(node)?);
            bfcs.push(Bfc::default());
            return Ok(None);
        }
        let natural = self.finish_table(k)?;
        if self.tables.len() != k + 1 {
            return Err(FlowError::InvalidSnapshot);
        }
        self.tables.pop();
        Ok(Some(natural))
    }

    /// Record the natural content height of the cell or caption just laid
    /// out.
    pub(super) fn table_item_done(
        &mut self,
        k: usize,
        node: usize,
        natural: f64,
    ) -> Result<(), FlowError> {
        let target = self.tables[k]
            .current
            .take()
            .ok_or(FlowError::InvalidSnapshot)?;
        match target {
            Target::Cell(i) => {
                if self.tables[k].structure.cells[i].node != node {
                    return Err(FlowError::InvalidSnapshot);
                }
                let baseline = self.content_baseline(node, false)?;
                let cell = &mut self.tables[k].structure.cells[i];
                cell.natural = natural;
                cell.baseline = baseline;
            }
            Target::Caption(i) => {
                let caption = &mut self.tables[k].structure.captions[i];
                if caption.node != node {
                    return Err(FlowError::InvalidSnapshot);
                }
                caption.natural = natural;
            }
        }
        Ok(())
    }

    /// Size the rows, place every part and return the content height.
    fn finish_table(&mut self, k: usize) -> Result<f64, FlowError> {
        let t = self.tables[k].table;
        let (h_space, v_space) = self.tables[k].spacing;
        let width = self.tables[k].width;
        let mut s = std::mem::replace(
            &mut self.tables[k].structure,
            Structure {
                groups: Vec::new(),
                rows: Vec::new(),
                cells: Vec::new(),
                captions: Vec::new(),
                columns: 0,
            },
        );
        // Cell border-box heights (a specified height is a minimum).
        let mut heights = Vec::with_capacity(s.cells.len());
        for cell in &s.cells {
            self.work.charge(1)?;
            let n = &self.nodes[cell.node];
            let extras = cell.vertical_extras();
            let constraint = HeightConstraint::new(n.style, None, CssPx::new(extras)?)?;
            let content = constraint
                .used(CssPx::new(cell.natural.max(0.0))?)
                .get()
                .max(cell.natural);
            heights.push(content + extras);
        }
        // Rows: single-row cells and baselines first (CSS 2.2 §17.5.3).
        let count = s.rows.len();
        let mut row_h: Vec<f64> = s.rows.iter().map(|r| r.minimum).collect();
        let mut above = vec![0.0_f64; count];
        let mut below = vec![0.0_f64; count];
        let ascent = |cell: &Cell, height: f64| {
            let top = cell.measure.border.top.get() + cell.measure.padding.top.get();
            top + cell.baseline.unwrap_or(height - cell.vertical_extras())
        };
        for (cell, &h) in s.cells.iter().zip(&heights) {
            if cell.align == Align::Baseline {
                let a = ascent(cell, h);
                above[cell.row] = above[cell.row].max(a);
                if cell.rows == 1 {
                    below[cell.row] = below[cell.row].max(h - a);
                }
            }
            if cell.rows == 1 {
                row_h[cell.row] = row_h[cell.row].max(h);
            }
        }
        for r in 0..count {
            row_h[r] = row_h[r].max(above[r] + below[r]);
        }
        let mut order: Vec<usize> = (0..s.cells.len())
            .filter(|&i| s.cells[i].rows > 1)
            .collect();
        order.sort_by_key(|&i| s.cells[i].rows);
        for i in order {
            self.work.charge(s.cells[i].rows)?;
            let c = &s.cells[i];
            let range = c.row..c.row + c.rows;
            let have = row_h[range.clone()].iter().sum::<f64>() + v_space * (c.rows as f64 - 1.0);
            let weights = row_h[range.clone()].to_vec();
            share(&mut row_h[range], &weights, heights[i] - have);
        }
        // A taller specified table height is shared by the rows.
        let captions: f64 = s
            .captions
            .iter()
            .map(|c| {
                let m = &c.measure;
                m.margin.top.get()
                    + m.border.top.get()
                    + m.padding.top.get()
                    + c.natural
                    + m.padding.bottom.get()
                    + m.border.bottom.get()
                    + m.margin.bottom.get()
            })
            .sum();
        let grid = |row_h: &[f64]| row_h.iter().sum::<f64>() + gaps(count, v_space);
        let constraint = self.nodes[t].constraint()?;
        if count > 0 {
            if let Some(h) = constraint.specified() {
                let extra = constraint.used(h).get() - captions - grid(&row_h);
                let weights = row_h.clone();
                share(&mut row_h, &weights, extra);
            }
        }
        // Row tops in the table's content box, captions above and below.
        let mut y = 0.0;
        for caption in s.captions.iter_mut().filter(|c| !c.bottom) {
            y = self.place_caption(caption, y)?;
        }
        let grid_top = y;
        let mut tops = Vec::with_capacity(count);
        if count > 0 {
            y += v_space;
            for &h in &row_h {
                tops.push(y);
                y += h + v_space;
            }
        }
        for caption in s.captions.iter_mut().filter(|c| c.bottom) {
            y = self.place_caption(caption, y)?;
        }
        let inner = (width - 2.0 * h_space).max(0.0);
        let lead = if s.columns == 0 { 0.0 } else { h_space };
        let row_span = |a: usize, b: usize| tops[b - 1] + row_h[b - 1] - tops[a];
        // Groups and rows.
        for group in &s.groups {
            self.work.charge(1)?;
            let (a, b) = group.rows;
            let (top, height) = if a < b {
                (tops[a], row_span(a, b))
            } else {
                (grid_top, 0.0)
            };
            if let Some(g) = group.node {
                self.set_box(g, (lead, top), inner, height)?;
            }
            for r in a..b {
                let origin = match group.node {
                    Some(_) => (0.0, tops[r] - top),
                    None => (lead, tops[r]),
                };
                self.set_box(s.rows[r].node, origin, inner, row_h[r])?;
            }
        }
        // Cells, relative to their first row.
        for (cell, &h) in s.cells.iter_mut().zip(&heights) {
            self.work.charge(1)?;
            let slot = row_span(cell.row, cell.row + cell.rows);
            let content = h - cell.vertical_extras();
            let free = (slot - h).max(0.0);
            let offset = match cell.align {
                Align::Top => 0.0,
                Align::Middle => free / 2.0,
                Align::Bottom => free,
                Align::Baseline => (above[cell.row] - ascent(cell, h)).clamp(0.0, free),
            };
            let x = match self.tables[k].direction {
                Direction::Ltr => self.tables[k].lefts[cell.column],
                Direction::Rtl => self.tables[k].lefts[cell.column + cell.columns - 1],
            };
            let n = &mut self.nodes[cell.node];
            let mut measure = cell.measure;
            measure.padding.top = CssPx::new(measure.padding.top.get() + offset)?;
            n.measure = measure;
            n.content_height = CssPx::new((content + free - offset).max(0.0))?;
            n.border_height = CssPx::new(slot.max(h))?;
            n.pos = (x - lead, 0.0);
            n.top_resolved = true;
        }
        // Columns have no boxes of their own yet.
        let end = self.nodes[t].end;
        let mut c = t + 1;
        while c < end {
            self.work.charge(1)?;
            if self.nodes[c].table == Some(TablePart::Column) {
                let next = self.nodes[c].end;
                for i in c..next {
                    self.set_box(i, (0.0, grid_top), 0.0, 0.0)?;
                }
                c = next;
            } else {
                c += 1;
            }
        }
        s.rows.clear();
        self.tables[k].structure = s;
        // The table's height is at least its content (CSS 2.2 §17.5.3).
        self.nodes[t].height = Some(HeightConstraint::solved(None));
        Ok(y)
    }

    /// Place a caption at `y` in the table's content box; returns the next y.
    fn place_caption(&mut self, caption: &mut Caption, y: f64) -> Result<f64, FlowError> {
        let m = caption.measure;
        let n = &mut self.nodes[caption.node];
        n.measure = m;
        n.content_height = CssPx::new(caption.natural.max(0.0))?;
        n.border_height = n.content_height.checked_add(n.vertical_extras()?)?;
        n.pos = (m.margin.left.get(), y + m.margin.top.get());
        n.top_resolved = true;
        Ok(y + m.margin.top.get() + n.border_height.get() + m.margin.bottom.get())
    }

    /// Give a row, row group or column a border box with no edges.
    fn set_box(
        &mut self,
        i: usize,
        pos: (f64, f64),
        width: f64,
        height: f64,
    ) -> Result<(), FlowError> {
        let n = &mut self.nodes[i];
        n.measure = BlockMeasure {
            content_width: CssPx::new(width.max(0.0))?,
            margin: Edges::default(),
            padding: Edges::default(),
            border: Edges::default(),
        };
        n.height = Some(HeightConstraint::solved(Some(CssPx::new(height.max(0.0))?)));
        n.content_height = CssPx::new(height.max(0.0))?;
        n.border_height = n.content_height;
        n.pos = pos;
        n.top_resolved = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(column: usize, columns: usize, min: f64, max: f64) -> Cell {
        Cell {
            node: 0,
            row: 0,
            column,
            rows: 1,
            columns,
            align: Align::Baseline,
            min,
            max,
            measure: EMPTY,
            natural: 0.0,
            baseline: None,
        }
    }

    #[test]
    fn spanning_cells_widen_columns_by_their_maximums() {
        let cells = [
            cell(0, 1, 10.0, 30.0),
            cell(1, 1, 10.0, 10.0),
            cell(0, 2, 60.0, 100.0),
        ];
        let (min, max) = column_bounds(&cells, 2, 0.0);
        // 40 extra minimum shared 30:10; 60 extra maximum the same way.
        assert_eq!(min, vec![40.0, 20.0]);
        assert_eq!(max, vec![75.0, 25.0]);
    }

    #[test]
    fn automatic_widths_interpolate_then_grow_by_maximums() {
        let (min, max) = ([10.0, 10.0], [30.0, 10.0]);
        assert_eq!(auto_widths(&min, &max, 15.0), vec![10.0, 10.0]);
        assert_eq!(auto_widths(&min, &max, 30.0), vec![20.0, 10.0]);
        assert_eq!(auto_widths(&min, &max, 80.0), vec![60.0, 20.0]);
    }
}
