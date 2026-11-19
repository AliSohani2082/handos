//! What the daemon needs from the desktop: an overlay to draw on and a pointer to drive.
//! Keyboard, clipboard, capture and window control come in later milestones.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(any(test, feature = "fake"))]
pub mod fake;

pub use handos_render::{DisplayList, Prim};

/// Stable for as long as the output stays connected; never reused within one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputId(pub u32);

/// A rectangle in the compositor's logical layout coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutputInfo {
    pub id: OutputId,
    /// Connector name such as `eDP-1`, as `mapping.target` and `overlay.outputs` name it.
    pub name: String,
    pub logical_rect: Rect,
    /// Fractional on outputs with fractional scaling, which M1 has not tested.
    pub scale: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Left,
    Right,
}

/// What a backend can do, reported by `handos doctor` and checked at startup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Features {
    /// Pointer motion can be placed at absolute coordinates on a chosen output.
    pub pointer_absolute: bool,
    /// Click-through surfaces above every window are available.
    pub overlay: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("output {0:?} is not connected")]
    UnknownOutput(OutputId),
    #[error("the compositor does not offer {0}")]
    Unsupported(&'static str),
    #[error("GPU error: {0}")]
    Gpu(String),
    #[error("Wayland error: {0}")]
    Wayland(String),
}

pub trait Overlay {
    fn outputs(&self) -> Vec<OutputInfo>;
    /// Draws `list` on `output` at the next frame, replacing whatever was there.
    fn draw(&mut self, output: OutputId, list: &DisplayList) -> Result<(), PlatformError>;
    /// Removes everything from `output` once and stops committing to it until the next `draw`.
    fn clear(&mut self, output: OutputId);
}

/// Requests are queued and sent together; a backend flushes them once per loop iteration.
pub trait Input {
    /// Moves the pointer to `(x, y)` in `output`'s logical pixels.
    fn pointer_abs(&mut self, output: OutputId, x: f64, y: f64);
    fn button(&mut self, btn: Button, pressed: bool);
    /// Vertical scroll in logical pixels; positive moves the content up.
    fn axis(&mut self, dy: f64);
    /// Ends a finger scroll so the compositor can start kinetic scrolling.
    fn axis_stop(&mut self);
}
