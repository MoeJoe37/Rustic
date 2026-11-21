//! Layer scenes (UI-05, ADR 0064 step 2): scrolling re-composites cached
//! content instead of repainting it.
//!
//! A scene is a stack of layers in paint order. Every layer's display
//! list is at unscrolled page coordinates over the page's whole
//! scrollable area and is rasterized lazily in [`TILE_SIZE`] tiles that
//! stay cached between frames; a content change drops only the tiles
//! [`crate::damage`] reports for that layer. **Page** layers move with
//! the viewport. A **scroller** layer holds one scroll container's
//! contents, painted without the container's clip, and is composited
//! through its scrollport at the container's own scroll offset. The first
//! layer is opaque; the others, and the **overlay** (fixed and
//! viewport-sticky boxes at window coordinates), are over transparent
//! canvases and composited source-over in order.
//!
//! The scene's output is the reference for scrolled pages: compositing
//! layers can differ from one-pass rasterization by rounding (at most
//! 1/255 per channel where layers overlap), and an incremental scene
//! frame equals a fresh scene of the same frame exactly.

use std::collections::HashMap;

use rustic_base::CancellationToken;
use rustic_layout::DeviceScale;
use rustic_paint::DisplayList;

use crate::raster::{DeviceRect, RasterError, RasterLimits, Surface, TILE_SIZE};

/// What one scene frame cost.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SceneStats {
    /// Layer tiles rasterized for this frame.
    pub painted_tiles: usize,
    /// Visible layer tiles taken from the cache.
    pub reused_tiles: usize,
    /// Overlay pixels rasterized (the window, when there is an overlay).
    pub overlay_pixels: u64,
}

/// How a layer moves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayerKind {
    /// It moves with the viewport.
    Page,
    /// A scroll container's contents: shown only inside `port` (the
    /// scrollport within its ancestors' clips, in unscrolled page device
    /// pixels), shifted by the container's `scroll` offset.
    Scroller {
        /// The visible area in unscrolled page device pixels.
        port: DeviceRect,
        /// The container's scroll offset in device pixels.
        scroll: (u32, u32),
    },
}

/// One layer of a frame.
#[derive(Clone, Copy, Debug)]
pub struct SceneLayer<'a> {
    /// The layer's list: unscrolled, its viewport the page's whole
    /// scrollable area, over a transparent canvas unless first.
    pub list: &'a DisplayList,
    /// How it moves.
    pub kind: LayerKind,
}

/// A frame's inputs.
#[derive(Debug)]
pub struct SceneFrame<'a> {
    /// The layers in paint order; the first is opaque and a page layer.
    pub layers: Vec<SceneLayer<'a>>,
    /// The overlay at window coordinates over a transparent canvas.
    pub overlay: Option<&'a DisplayList>,
    /// The viewport's scroll offset in device pixels.
    pub offset: (u32, u32),
    /// The window size in device pixels.
    pub size: (u32, u32),
}

#[derive(Debug)]
struct Cached {
    list: DisplayList,
    kind: LayerKind,
    tiles: HashMap<(u32, u32), Surface>,
}

/// Cached tiles of each layer and the lists they were painted from.
#[derive(Debug)]
pub struct Scene {
    scale: DeviceScale,
    limits: RasterLimits,
    layers: Vec<Cached>,
}

fn overlaps(a: DeviceRect, b: DeviceRect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

fn intersect(a: DeviceRect, b: DeviceRect) -> Option<DeviceRect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    (x0 < x1 && y0 < y1).then(|| DeviceRect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}

/// Whether a cached layer's tiles serve `next`: the same kind (and for a
/// scroller the same port); the scroll offset only moves what is shown.
fn same_layer(cached: LayerKind, next: LayerKind) -> bool {
    match (cached, next) {
        (LayerKind::Page, LayerKind::Page) => true,
        (LayerKind::Scroller { port: a, .. }, LayerKind::Scroller { port: b, .. }) => a == b,
        _ => false,
    }
}

/// Source-over one premultiplied pixel.
fn blend(bottom: &mut [u8], top: &[u8]) {
    let keep = 255 - u32::from(top[3]);
    if keep == 255 {
        return;
    }
    for channel in 0..4 {
        let below = (u32::from(bottom[channel]) * keep + 127) / 255;
        bottom[channel] = (u32::from(top[channel]) + below).min(255) as u8;
    }
}

impl Scene {
    /// An empty scene rasterizing at `scale` within `limits`.
    pub fn new(scale: DeviceScale, limits: RasterLimits) -> Self {
        Self {
            scale,
            limits,
            layers: Vec::new(),
        }
    }

    /// Render one frame. On failure every cache is dropped, so the next
    /// frame starts afresh.
    pub fn submit(
        &mut self,
        frame: SceneFrame<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(Surface, SceneStats), RasterError> {
        let result = self.render(frame, cancellation);
        if result.is_err() {
            self.layers.clear();
        }
        result
    }

    /// Bring layer `index`'s cache up to `list`, dropping what changed.
    fn refresh(&mut self, index: usize, layer: SceneLayer<'_>) -> Result<(), RasterError> {
        let size = crate::surface_size(layer.list, self.scale)?;
        let reusable = match self.layers.get(index) {
            Some(cached) => {
                same_layer(cached.kind, layer.kind)
                    && crate::surface_size(&cached.list, self.scale)? == size
            }
            None => false,
        };
        if !reusable {
            let fresh = Cached {
                list: layer.list.clone(),
                kind: layer.kind,
                tiles: HashMap::new(),
            };
            if index < self.layers.len() {
                self.layers[index] = fresh;
            } else {
                self.layers.push(fresh);
            }
            return Ok(());
        }
        let cached = &mut self.layers[index];
        if &cached.list != layer.list {
            for region in crate::damage(&cached.list, layer.list, self.scale)? {
                cached.tiles.retain(|&(tx, ty), _| {
                    !overlaps(
                        region,
                        DeviceRect {
                            x: tx * TILE_SIZE,
                            y: ty * TILE_SIZE,
                            width: TILE_SIZE,
                            height: TILE_SIZE,
                        },
                    )
                });
            }
            cached.list = layer.list.clone();
        }
        Ok(())
    }

    fn render(
        &mut self,
        frame: SceneFrame<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(Surface, SceneStats), RasterError> {
        let (w, h) = frame.size;
        let (ox, oy) = frame.offset;
        if !matches!(frame.layers.first(), Some(layer) if layer.kind == LayerKind::Page) {
            return Err(RasterError::InvalidList);
        }
        self.layers.truncate(frame.layers.len());
        let mut output = Surface::new(w, h, self.limits)?;
        let mut stats = SceneStats::default();
        for (index, layer) in frame.layers.iter().enumerate() {
            self.refresh(index, *layer)?;
            let (page_w, page_h) = crate::surface_size(layer.list, self.scale)?;
            // The window in this layer's page coordinates (`from`), and
            // the window pixels it covers (`to`, same size).
            let window = DeviceRect {
                x: ox,
                y: oy,
                width: w,
                height: h,
            };
            let (from, to) = match layer.kind {
                LayerKind::Page => (
                    window,
                    DeviceRect {
                        x: 0,
                        y: 0,
                        width: w,
                        height: h,
                    },
                ),
                LayerKind::Scroller { port, scroll } => {
                    let Some(shown) = intersect(port, window) else {
                        continue;
                    };
                    let from = DeviceRect {
                        x: shown
                            .x
                            .checked_add(scroll.0)
                            .ok_or(RasterError::SurfaceSize)?,
                        y: shown
                            .y
                            .checked_add(scroll.1)
                            .ok_or(RasterError::SurfaceSize)?,
                        ..shown
                    };
                    let to = DeviceRect {
                        x: shown.x - ox,
                        y: shown.y - oy,
                        ..shown
                    };
                    (from, to)
                }
            };
            let fits = from.x.checked_add(from.width).is_some_and(|r| r <= page_w)
                && from.y.checked_add(from.height).is_some_and(|b| b <= page_h);
            if !fits {
                return Err(RasterError::SurfaceSize);
            }
            for ty in from.y / TILE_SIZE..(from.y + from.height).div_ceil(TILE_SIZE) {
                for tx in from.x / TILE_SIZE..(from.x + from.width).div_ceil(TILE_SIZE) {
                    let rect = DeviceRect {
                        x: tx * TILE_SIZE,
                        y: ty * TILE_SIZE,
                        width: TILE_SIZE.min(page_w - tx * TILE_SIZE),
                        height: TILE_SIZE.min(page_h - ty * TILE_SIZE),
                    };
                    let Some(part) = intersect(rect, from) else {
                        continue;
                    };
                    let cached = &mut self.layers[index];
                    if let std::collections::hash_map::Entry::Vacant(slot) =
                        cached.tiles.entry((tx, ty))
                    {
                        slot.insert(crate::rasterize_region(
                            layer.list,
                            self.scale,
                            rect,
                            self.limits,
                            cancellation,
                        )?);
                        stats.painted_tiles += 1;
                    } else {
                        stats.reused_tiles += 1;
                    }
                    let tile = &cached.tiles[&(tx, ty)];
                    let pixels = output.pixels_mut_internal();
                    for y in part.y..part.y + part.height {
                        for x in part.x..part.x + part.width {
                            let source =
                                (((y - rect.y) * tile.width() + (x - rect.x)) * 4) as usize;
                            let (dx, dy) = (x - from.x + to.x, y - from.y + to.y);
                            let target = ((dy * w + dx) * 4) as usize;
                            let top = &tile.premultiplied()[source..source + 4];
                            if index == 0 {
                                pixels[target..target + 4].copy_from_slice(top);
                            } else {
                                blend(&mut pixels[target..target + 4], top);
                            }
                        }
                    }
                }
            }
        }
        if let Some(overlay) = frame.overlay {
            let layer = crate::rasterize(overlay, self.scale, self.limits, cancellation)?;
            if (layer.width(), layer.height()) != (w, h) {
                return Err(RasterError::SurfaceSize);
            }
            let pixels = output.pixels_mut_internal();
            for (bottom, top) in pixels
                .chunks_exact_mut(4)
                .zip(layer.premultiplied().chunks_exact(4))
            {
                blend(bottom, top);
            }
            stats.overlay_pixels = u64::from(w) * u64::from(h);
        }
        Ok((output, stats))
    }
}
