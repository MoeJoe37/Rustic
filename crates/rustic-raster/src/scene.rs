//! Layer scenes (UI-05, ADR 0064 step 2, first part): a page scroll
//! re-composites cached content instead of repainting it.
//!
//! A scene has a **content layer**, the page's display list at unscrolled
//! coordinates over its whole scrollable area, rasterized lazily in
//! [`TILE_SIZE`] tiles that stay cached between frames, and an optional
//! **overlay layer** (fixed and viewport-sticky boxes) over a transparent
//! canvas at window coordinates. A frame copies the visible content tiles
//! at the viewport's scroll offset (whole device pixels) and composites
//! the overlay over them. Content changes invalidate only the tiles
//! [`crate::damage`] reports. The scene's output is the reference for
//! scrolled frames: compositing the overlay separately can differ from a
//! one-pass rasterization by rounding (at most 1/255 per channel), and an
//! incremental scene frame equals a full rebuild of the same scene exactly.

use std::collections::HashMap;

use rustic_base::CancellationToken;
use rustic_layout::DeviceScale;
use rustic_paint::DisplayList;

use crate::raster::{DeviceRect, RasterError, RasterLimits, Surface, TILE_SIZE};

/// What one scene frame cost.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SceneStats {
    /// Content tiles rasterized for this frame.
    pub painted_tiles: usize,
    /// Visible content tiles taken from the cache.
    pub reused_tiles: usize,
    /// Overlay pixels rasterized (the window, when there is an overlay).
    pub overlay_pixels: u64,
}

/// A frame's inputs.
#[derive(Debug)]
pub struct SceneFrame<'a> {
    /// The content layer: unscrolled, its viewport the whole scrollable
    /// area.
    pub content: &'a DisplayList,
    /// The overlay layer at window coordinates over a transparent canvas,
    /// or `None`.
    pub overlay: Option<&'a DisplayList>,
    /// The viewport's scroll offset in device pixels.
    pub offset: (u32, u32),
    /// The window size in device pixels.
    pub size: (u32, u32),
}

/// Cached content tiles and the list they were painted from.
#[derive(Debug)]
pub struct Scene {
    scale: DeviceScale,
    limits: RasterLimits,
    content: Option<DisplayList>,
    tiles: HashMap<(u32, u32), Surface>,
}

fn overlaps(a: DeviceRect, b: DeviceRect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

/// Composite premultiplied `top` over `bottom`, source-over, rounding.
fn over(bottom: &mut [u8], top: &[u8]) {
    for (b, t) in bottom.chunks_exact_mut(4).zip(top.chunks_exact(4)) {
        let keep = 255 - u32::from(t[3]);
        if keep == 255 {
            continue;
        }
        for channel in 0..4 {
            let below = (u32::from(b[channel]) * keep + 127) / 255;
            b[channel] = (u32::from(t[channel]) + below).min(255) as u8;
        }
    }
}

impl Scene {
    /// An empty scene rasterizing at `scale` within `limits`.
    pub fn new(scale: DeviceScale, limits: RasterLimits) -> Self {
        Self {
            scale,
            limits,
            content: None,
            tiles: HashMap::new(),
        }
    }

    /// Render one frame. On failure the cache is dropped, so the next
    /// frame starts afresh.
    pub fn submit(
        &mut self,
        frame: SceneFrame<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(Surface, SceneStats), RasterError> {
        let result = self.render(frame, cancellation);
        if result.is_err() {
            self.content = None;
            self.tiles.clear();
        }
        result
    }

    fn render(
        &mut self,
        frame: SceneFrame<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(Surface, SceneStats), RasterError> {
        let (content_w, content_h) = crate::surface_size(frame.content, self.scale)?;
        let (w, h) = frame.size;
        let (ox, oy) = frame.offset;
        let fits = ox.checked_add(w).is_some_and(|r| r <= content_w)
            && oy.checked_add(h).is_some_and(|b| b <= content_h);
        if !fits {
            return Err(RasterError::SurfaceSize);
        }
        // Invalidate what changed since the cached list.
        match &self.content {
            Some(old) if crate::surface_size(old, self.scale)? == (content_w, content_h) => {
                if old != frame.content {
                    for region in crate::damage(old, frame.content, self.scale)? {
                        self.tiles.retain(|&(tx, ty), _| {
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
                }
            }
            _ => self.tiles.clear(),
        }
        self.content = Some(frame.content.clone());
        let mut output = Surface::new(w, h, self.limits)?;
        let mut stats = SceneStats::default();
        let view = DeviceRect {
            x: ox,
            y: oy,
            width: w,
            height: h,
        };
        for ty in oy / TILE_SIZE..(oy + h).div_ceil(TILE_SIZE) {
            for tx in ox / TILE_SIZE..(ox + w).div_ceil(TILE_SIZE) {
                let rect = DeviceRect {
                    x: tx * TILE_SIZE,
                    y: ty * TILE_SIZE,
                    width: TILE_SIZE.min(content_w - tx * TILE_SIZE),
                    height: TILE_SIZE.min(content_h - ty * TILE_SIZE),
                };
                if !overlaps(rect, view) {
                    continue;
                }
                if !self.tiles.contains_key(&(tx, ty)) {
                    let tile = crate::rasterize_region(
                        frame.content,
                        self.scale,
                        rect,
                        self.limits,
                        cancellation,
                    )?;
                    self.tiles.insert((tx, ty), tile);
                    stats.painted_tiles += 1;
                } else {
                    stats.reused_tiles += 1;
                }
                let tile = &self.tiles[&(tx, ty)];
                // Copy the part of the tile inside the view.
                let x0 = rect.x.max(ox);
                let x1 = (rect.x + rect.width).min(ox + w);
                let y0 = rect.y.max(oy);
                let y1 = (rect.y + rect.height).min(oy + h);
                let row_bytes = ((x1 - x0) * 4) as usize;
                for y in y0..y1 {
                    let source = (((y - rect.y) * tile.width() + (x0 - rect.x)) * 4) as usize;
                    let target = (((y - oy) * w + (x0 - ox)) * 4) as usize;
                    output.pixels_mut_internal()[target..target + row_bytes]
                        .copy_from_slice(&tile.premultiplied()[source..source + row_bytes]);
                }
            }
        }
        if let Some(overlay) = frame.overlay {
            let layer = crate::rasterize(overlay, self.scale, self.limits, cancellation)?;
            if (layer.width(), layer.height()) != (w, h) {
                return Err(RasterError::SurfaceSize);
            }
            over(output.pixels_mut_internal(), layer.premultiplied());
            stats.overlay_pixels = u64::from(w) * u64::from(h);
        }
        Ok((output, stats))
    }
}
