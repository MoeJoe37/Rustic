//! Damage between two display lists: which tiles of the surface can differ
//! when `old` is replaced by `new`, so that repainting only those tiles over
//! the old surface gives exactly the new surface.
//!
//! Each list flattens to its fills, each keyed by what it paints (its shape
//! and side-table data resolved, color and clips) and by the chain of groups,
//! clips and transforms it paints inside. The fills common to the start and
//! the end of both lists paint the same pixels in the same order; every
//! other fill, from either list, damages the device pixels it can touch,
//! grown by the reach of the filters around it. Source box and node ids are
//! not part of the key, so relayout that moves nothing paints nothing.

use crate::raster::{
    Bounds, DeviceRect, RasterError, TILE_SIZE, blur_reach, chain, device_factor, effect_margin,
    filter_reach, group_scale, local_bounds, project, stretch, surface_size,
};
use rustic_layout::{Clip, CssRect, DeviceScale};
use rustic_paint::{
    BlendMode, BoxShadow, DisplayCommand, DisplayList, FilterOp, Homography, Image, ItemRole, Path,
    PathPoint, RoundedRect, Shape, Srgba,
};
use std::sync::Arc;

/// A shape with its side-table data in place of indices.
#[derive(PartialEq)]
enum Resolved<'a> {
    /// A shape with no indices (rectangle, ring or quad).
    Plain(Shape),
    Path(&'a Path),
    Rounded(&'a RoundedRect),
    RoundedRing(&'a RoundedRect, &'a RoundedRect),
    Shadow(&'a BoxShadow),
    Image(CssRect, &'a Arc<Image>),
}

/// A layer a fill paints inside.
#[derive(PartialEq)]
enum Scope<'a> {
    Group {
        opacity: f64,
        blend: BlendMode,
        filter: Option<&'a [FilterOp]>,
    },
    Clip(Resolved<'a>),
    Transform {
        matrix: Homography,
        clip: Clip,
        rounded: Vec<&'a RoundedRect>,
    },
}

/// One fill of a list, keyed by what it paints.
struct Entry<'a> {
    /// Index of the innermost scope in [`Flat::scopes`].
    scope: Option<usize>,
    role: ItemRole,
    shape: Resolved<'a>,
    color: Srgba,
    clip: Clip,
    rounded: Vec<&'a RoundedRect>,
    /// Device pixels the fill can change, effects included.
    bounds: Option<Bounds>,
}

/// A list flattened to its fills and the scopes they paint in.
struct Flat<'a> {
    /// Each scope with its parent.
    scopes: Vec<(Scope<'a>, Option<usize>)>,
    entries: Vec<Entry<'a>>,
}

fn resolve<'a>(shape: &Shape, list: &'a DisplayList) -> Result<Resolved<'a>, RasterError> {
    let rounded = |i: usize| list.rounded().get(i).ok_or(RasterError::InvalidList);
    Ok(match *shape {
        Shape::Rect(_) | Shape::Ring { .. } | Shape::Quad(_) => Resolved::Plain(*shape),
        Shape::Path(i) => Resolved::Path(list.paths().get(i).ok_or(RasterError::InvalidList)?),
        Shape::Rounded(i) => Resolved::Rounded(rounded(i)?),
        Shape::RoundedRing { outer, inner } => {
            Resolved::RoundedRing(rounded(outer)?, rounded(inner)?)
        }
        Shape::Shadow(i) => {
            Resolved::Shadow(list.shadows().get(i).ok_or(RasterError::InvalidList)?)
        }
        Shape::Image { rect, image } => Resolved::Image(
            rect,
            list.images().get(image).ok_or(RasterError::InvalidList)?,
        ),
    })
}

fn rounded_chain(
    list: &DisplayList,
    first: Option<usize>,
) -> Result<Vec<&RoundedRect>, RasterError> {
    Ok(chain(list.clips(), first)?
        .into_iter()
        .map(|node| &node.shape)
        .collect())
}

/// Local bounds `(left, top, right, bottom)` of a rectangle.
fn rect_bounds(r: CssRect) -> Bounds {
    let (x, y) = (r.origin().x.get(), r.origin().y.get());
    (
        x,
        y,
        x + r.size().width().get(),
        y + r.size().height().get(),
    )
}

fn corners((x0, y0, x1, y1): Bounds) -> [PathPoint; 4] {
    let on = |x, y| PathPoint {
        x,
        y,
        on_curve: true,
    };
    [on(x0, y0), on(x1, y0), on(x1, y1), on(x0, y1)]
}

/// The device pixels a fill of `shape` can change under `map`, before
/// filters: its local bounds within its axis clip, mapped, grown by a
/// shadow's blur.
fn fill_bounds(shape: &Resolved<'_>, clip: Clip, map: &Homography) -> Option<Bounds> {
    let (local, sigma) = match *shape {
        Resolved::Plain(Shape::Rect(r)) | Resolved::Image(r, _) => (rect_bounds(r), 0.0),
        Resolved::Plain(Shape::Ring { outer, .. }) => (rect_bounds(outer), 0.0),
        Resolved::Plain(Shape::Quad(points)) => {
            let mut b = (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            );
            for p in points {
                let (x, y) = (p.x.get(), p.y.get());
                b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
            }
            (b, 0.0)
        }
        Resolved::Plain(_) => return None,
        Resolved::Path(path) => {
            // The control points' hull contains the curves.
            let mut b = (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            );
            for p in path.contours().iter().flatten() {
                b = (b.0.min(p.x), b.1.min(p.y), b.2.max(p.x), b.3.max(p.y));
            }
            (b, 0.0)
        }
        Resolved::Rounded(r) | Resolved::RoundedRing(r, _) => (rect_bounds(r.rect), 0.0),
        Resolved::Shadow(s) if s.inset => (rect_bounds(s.clip.rect), 0.0),
        Resolved::Shadow(s) => (rect_bounds(s.shape?.rect), s.sigma),
    };
    if !(local.0 < local.2 && local.1 < local.3) {
        return None;
    }
    let points = project(&corners(local), map, Some(local_bounds(clip)));
    let mut b = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for (x, y) in points {
        b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
    }
    if !(b.0 <= b.2 && b.1 <= b.3) {
        return None;
    }
    let reach = if sigma > 0.0 {
        let device = sigma * stretch(map, &corners(local));
        blur_reach(device) as f64
    } else {
        0.0
    };
    Some((b.0 - reach, b.1 - reach, b.2 + reach, b.3 + reach))
}

fn flatten(list: &DisplayList, factor: f64) -> Result<Flat<'_>, RasterError> {
    let mut flat = Flat {
        scopes: Vec::new(),
        entries: Vec::new(),
    };
    // Innermost open scope, its map and filter reach, innermost last.
    let mut open: Vec<(Option<usize>, Option<Homography>, usize)> = Vec::new();
    let mut scope: Option<usize> = None;
    let mut reach = 0usize;
    let base = Homography::scale(factor);
    for command in list.commands() {
        let map = open.iter().rev().find_map(|o| o.1);
        match command {
            DisplayCommand::Fill(item) => {
                let shape = resolve(&item.shape, list)?;
                let bounds =
                    fill_bounds(&shape, item.clip, map.as_ref().unwrap_or(&base)).map(|b| {
                        // One pixel more for antialiasing and rounding.
                        let pad = reach as f64 + 1.0;
                        (b.0 - pad, b.1 - pad, b.2 + pad, b.3 + pad)
                    });
                flat.entries.push(Entry {
                    scope,
                    role: item.role,
                    shape,
                    color: item.color,
                    clip: item.clip,
                    rounded: rounded_chain(list, item.rounded_clip)?,
                    bounds,
                });
            }
            DisplayCommand::BeginGroup {
                opacity,
                blend,
                filter,
                ..
            } => {
                let ops = match filter {
                    None => None,
                    Some(f) => Some(
                        list.filters()
                            .get(*f)
                            .ok_or(RasterError::InvalidList)?
                            .as_slice(),
                    ),
                };
                let scale = group_scale(map.as_ref(), factor);
                let r = ops.map_or(0, |ops| {
                    ops.iter()
                        .map(|op| filter_reach(op, scale))
                        .fold(0usize, usize::saturating_add)
                });
                flat.scopes.push((
                    Scope::Group {
                        opacity: *opacity,
                        blend: *blend,
                        filter: ops,
                    },
                    scope,
                ));
                open.push((scope, None, r));
                reach = reach.saturating_add(r);
                scope = Some(flat.scopes.len() - 1);
            }
            DisplayCommand::BeginClip { shape, .. } => {
                flat.scopes
                    .push((Scope::Clip(resolve(shape, list)?), scope));
                open.push((scope, None, 0));
                scope = Some(flat.scopes.len() - 1);
            }
            DisplayCommand::BeginTransform { transform, .. } => {
                let node = list
                    .transforms()
                    .get(*transform)
                    .ok_or(RasterError::InvalidList)?;
                let inner = map.unwrap_or(base).then_after(&node.matrix);
                flat.scopes.push((
                    Scope::Transform {
                        matrix: node.matrix,
                        clip: node.clip,
                        rounded: rounded_chain(list, node.rounded_clip)?,
                    },
                    scope,
                ));
                open.push((scope, Some(inner), 0));
                scope = Some(flat.scopes.len() - 1);
            }
            DisplayCommand::EndGroup | DisplayCommand::EndClip | DisplayCommand::EndTransform => {
                let (outer, _, r) = open.pop().ok_or(RasterError::InvalidList)?;
                scope = outer;
                reach = reach.saturating_sub(r);
            }
        }
    }
    if !open.is_empty() {
        return Err(RasterError::InvalidList);
    }
    Ok(flat)
}

/// Whether two scope chains are the same layers in the same nesting.
fn same_scope(a: &Flat<'_>, mut i: Option<usize>, b: &Flat<'_>, mut j: Option<usize>) -> bool {
    loop {
        match (i, j) {
            (None, None) => return true,
            (Some(x), Some(y)) => {
                let (sa, pa) = &a.scopes[x];
                let (sb, pb) = &b.scopes[y];
                if sa != sb {
                    return false;
                }
                (i, j) = (*pa, *pb);
            }
            _ => return false,
        }
    }
}

fn same(a: &Flat<'_>, i: usize, b: &Flat<'_>, j: usize) -> bool {
    let (x, y) = (&a.entries[i], &b.entries[j]);
    x.role == y.role
        && x.shape == y.shape
        && x.color == y.color
        && x.clip == y.clip
        && x.rounded == y.rounded
        && same_scope(a, x.scope, b, y.scope)
}

/// The tiles of the surface `new` rasterizes to at `scale` whose pixels
/// can differ from those of `old`, each as a rectangle of the [`TILE_SIZE`]
/// grid clipped to the surface, in row order. Repainting them (see
/// [`repaint`](crate::repaint)) over `old`'s surface gives exactly `new`'s.
/// A change of surface size, canvas color or filter margin damages every
/// tile.
pub fn damage(
    old: &DisplayList,
    new: &DisplayList,
    scale: DeviceScale,
) -> Result<Vec<DeviceRect>, RasterError> {
    let (width, height) = surface_size(new, scale)?;
    let factor = device_factor(scale)?;
    let tile = TILE_SIZE as usize;
    let (columns, rows) = (
        (width as usize).div_ceil(tile),
        (height as usize).div_ceil(tile),
    );
    let mut damaged = vec![false; columns * rows];
    let everything = surface_size(old, scale)? != (width, height)
        || old.canvas() != new.canvas()
        || effect_margin(old, factor)? != effect_margin(new, factor)?;
    if everything {
        damaged.fill(true);
    } else {
        let (a, b) = (flatten(old, factor)?, flatten(new, factor)?);
        let (la, lb) = (a.entries.len(), b.entries.len());
        let mut prefix = 0;
        while prefix < la.min(lb) && same(&a, prefix, &b, prefix) {
            prefix += 1;
        }
        let mut suffix = 0;
        while suffix < la.min(lb) - prefix && same(&a, la - 1 - suffix, &b, lb - 1 - suffix) {
            suffix += 1;
        }
        let changed = a.entries[prefix..la - suffix]
            .iter()
            .chain(&b.entries[prefix..lb - suffix]);
        for bounds in changed.filter_map(|e| e.bounds) {
            let (x0, y0, x1, y1) = bounds;
            let clamp = |v: f64, limit: u32| v.clamp(0.0, f64::from(limit)) as usize;
            let (cx0, cy0) = (clamp(x0.floor(), width), clamp(y0.floor(), height));
            let (cx1, cy1) = (clamp(x1.ceil(), width), clamp(y1.ceil(), height));
            if cx0 >= cx1 || cy0 >= cy1 {
                continue;
            }
            for row in cy0 / tile..cy1.div_ceil(tile) {
                for column in cx0 / tile..cx1.div_ceil(tile) {
                    damaged[row * columns + column] = true;
                }
            }
        }
    }
    let mut out = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            if damaged[row * columns + column] {
                let (x, y) = (column as u32 * TILE_SIZE, row as u32 * TILE_SIZE);
                out.push(DeviceRect {
                    x,
                    y,
                    width: TILE_SIZE.min(width - x),
                    height: TILE_SIZE.min(height - y),
                });
            }
        }
    }
    Ok(out)
}
