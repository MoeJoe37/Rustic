//! Host-facing lifecycle and configuration for an original Rust browser engine.

use std::error::Error;
use std::fmt;

mod classic_scripts;
mod frames;
pub mod history;
mod inline_scripts;
mod layout_reads;
mod lifecycle;
mod local_modules;
mod scripting;
mod tasks;
mod timelines;
mod window_tasks;

pub use classic_scripts::{
    ClassicFetchError, ClassicRequestId, ClassicScriptLimits, ClassicScriptMode,
    ClassicScriptRequest, ScriptResourceKind,
};
pub use frames::RenderFrame;
pub use inline_scripts::ScriptLoadError as InlineLoadError;
pub use inline_scripts::{ClassicScriptLoad, InlineScriptLoad, ScriptLoadError, ScriptPolicy};
pub use layout_reads::{LayoutGeometry, LayoutSource, MAX_LAYOUT_FLUSHES_PER_TURN};
pub use lifecycle::{DocumentLoad, DocumentStartError, Engine, EngineDocument};
pub use rustic_bindings::page::{
    AssistiveAction, BoxSizes, Composition, ComputedStyleRecord, ComputedStyleSnapshot,
    ConsoleLevel, ConsoleLog, ConsoleMessage, FetchHost, FormSubmission, FormSubmissions,
    InputEvent, KeyInput, LayoutReadError, Modifiers, NetworkFailure, POPUP_CAPACITY, PopupRequest,
    QuotaExceeded, ScriptBodyReader, ScriptRequest, ScriptResponse, StorageHost, StorageKind,
    read_script_body,
};
pub use scripting::{
    DocumentTurn, NodeLayout, ScriptFailure, ScriptQueueError, ScriptStop, ScriptTurn,
    ScriptedDocument, ScrollState, TaskLimits,
};
pub use tasks::TimerId;

/// Initial allocation budget for a headless viewport, in device pixels.
pub const MAX_VIEWPORT_PIXELS: u64 = 16_777_216;

/// Maximum width or height accepted by the initial viewport configuration.
pub const MAX_VIEWPORT_DIMENSION: u32 = 16_384;

/// A nonempty viewport whose dimensions and area satisfy the allocation budget.
///
/// This is configuration validation, not a renderer or a process memory limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Viewport {
    width: u32,
    height: u32,
}

impl Viewport {
    /// Validate dimensions before a future renderer allocates its surface.
    ///
    /// ```
    /// use rustic_engine::Viewport;
    /// let viewport = Viewport::new(1280, 720)?;
    /// assert_eq!(viewport.width(), 1280);
    /// # Ok::<(), rustic_engine::ViewportError>(())
    /// ```
    pub fn new(width: u32, height: u32) -> Result<Self, ViewportError> {
        if width == 0 || height == 0 {
            return Err(ViewportError::Empty);
        }
        if width > MAX_VIEWPORT_DIMENSION || height > MAX_VIEWPORT_DIMENSION {
            return Err(ViewportError::DimensionTooLarge);
        }
        if u64::from(width) * u64::from(height) > MAX_VIEWPORT_PIXELS {
            return Err(ViewportError::AreaTooLarge);
        }
        Ok(Self { width, height })
    }

    /// Width in device pixels.
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Height in device pixels.
    pub const fn height(self) -> u32 {
        self.height
    }
}

/// Why a viewport configuration was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewportError {
    /// At least one dimension is zero.
    Empty,
    /// At least one dimension exceeds the dimension budget.
    DimensionTooLarge,
    /// The pixel count exceeds the surface allocation budget.
    AreaTooLarge,
}

impl fmt::Display for ViewportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "viewport dimensions must be nonzero",
            Self::DimensionTooLarge => "viewport dimension exceeds the configured limit",
            Self::AreaTooLarge => "viewport pixel count exceeds the configured limit",
        })
    }
}

impl Error for ViewportError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_area_boundary() {
        let viewport = Viewport::new(4096, 4096).expect("boundary is valid");
        assert_eq!((viewport.width(), viewport.height()), (4096, 4096));
        assert_eq!(Viewport::new(4097, 4096), Err(ViewportError::AreaTooLarge));
    }

    #[test]
    fn rejects_empty_and_extreme_dimensions() {
        for (width, height) in [(0, 1), (1, 0), (0, 0)] {
            assert_eq!(Viewport::new(width, height), Err(ViewportError::Empty));
        }
        for (width, height) in [(u32::MAX, 1), (1, u32::MAX)] {
            assert_eq!(
                Viewport::new(width, height),
                Err(ViewportError::DimensionTooLarge)
            );
        }
    }

    #[test]
    fn accepts_dimension_boundary_and_rejects_next_value() {
        assert!(Viewport::new(MAX_VIEWPORT_DIMENSION, 1).is_ok());
        assert!(Viewport::new(1, MAX_VIEWPORT_DIMENSION).is_ok());
        assert_eq!(
            Viewport::new(MAX_VIEWPORT_DIMENSION + 1, 1),
            Err(ViewportError::DimensionTooLarge)
        );
    }
}
