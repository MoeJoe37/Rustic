//! Incremental frame submission (UI-05, ADR 0064 step 3): a sequence of
//! frames where each new display list repaints only the tiles [`damage`]
//! reports against the previous one, so every frame equals a full
//! rasterization of its list while unchanged regions cost nothing.
//!
//! [`damage`]: crate::damage

use rustic_base::CancellationToken;
use rustic_layout::DeviceScale;
use rustic_paint::DisplayList;

use crate::raster::{DeviceRect, RasterError, RasterLimits, Surface, TILE_SIZE};

/// What one submitted frame cost.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameStats {
    /// Tiles repainted.
    pub repainted_tiles: usize,
    /// Tiles in the surface.
    pub total_tiles: usize,
    /// Device pixels repainted.
    pub repainted_pixels: u64,
    /// Whether the whole surface was painted (the first frame, a new size
    /// or a change that damages everything).
    pub full: bool,
}

/// The last frame's display list and pixels.
#[derive(Debug)]
pub struct Frames {
    scale: DeviceScale,
    limits: RasterLimits,
    last: Option<(DisplayList, Surface)>,
}

fn tiles(width: u32, height: u32) -> usize {
    (width.div_ceil(TILE_SIZE) as usize) * (height.div_ceil(TILE_SIZE) as usize)
}

impl Frames {
    /// An empty sequence rasterizing at `scale` within `limits`.
    pub fn new(scale: DeviceScale, limits: RasterLimits) -> Self {
        Self {
            scale,
            limits,
            last: None,
        }
    }

    /// The current frame's pixels, once one was submitted.
    pub fn surface(&self) -> Option<&Surface> {
        self.last.as_ref().map(|(_, surface)| surface)
    }

    /// Make `list` the current frame, repainting only what changed since
    /// the previous one. On failure the previous frame stays current.
    pub fn submit(
        &mut self,
        list: DisplayList,
        cancellation: &CancellationToken,
    ) -> Result<FrameStats, RasterError> {
        let (width, height) = crate::surface_size(&list, self.scale)?;
        let total_tiles = tiles(width, height);
        let previous = self.last.take();
        let (surface, regions) = match previous {
            Some((old, mut surface)) if (surface.width(), surface.height()) == (width, height) => {
                let regions = match crate::damage(&old, &list, self.scale) {
                    Ok(regions) => regions,
                    Err(error) => {
                        self.last = Some((old, surface));
                        return Err(error);
                    }
                };
                // A partly repainted surface is no frame: on failure nothing
                // stays current, and the next frame paints fully.
                crate::repaint(
                    &mut surface,
                    &list,
                    self.scale,
                    &regions,
                    self.limits,
                    cancellation,
                )?;
                (surface, Some(regions))
            }
            previous => match crate::rasterize(&list, self.scale, self.limits, cancellation) {
                Ok(surface) => (surface, None),
                Err(error) => {
                    self.last = previous;
                    return Err(error);
                }
            },
        };
        let stats = match &regions {
            Some(regions) => FrameStats {
                repainted_tiles: regions.len(),
                total_tiles,
                repainted_pixels: regions
                    .iter()
                    .map(|r: &DeviceRect| u64::from(r.width) * u64::from(r.height))
                    .sum(),
                full: regions.len() == total_tiles,
            },
            None => FrameStats {
                repainted_tiles: total_tiles,
                total_tiles,
                repainted_pixels: u64::from(width) * u64::from(height),
                full: true,
            },
        };
        self.last = Some((list, surface));
        Ok(stats)
    }
}
