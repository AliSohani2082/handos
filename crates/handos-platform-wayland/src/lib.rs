//! The Wayland backend: one click-through layer-shell overlay and one wlr virtual pointer per
//! output, plus the Vulkan driver choice and the one Hyprland query M1 needs.
//!
//! [`Wayland`] is the SCTK state and implements `Overlay` and `Input`. Its event queue is handed
//! to the daemon's calloop loop as a [`WaylandSource`] whose callback dispatches into the
//! `Wayland` the daemon holds; [`dispatch`] does the same for callers without a loop.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod hyprland;
mod input;
mod overlay;
pub mod vulkan;

use std::time::Duration;

use handos_platform::{Features, OutputId, OutputInfo, PlatformError, Rect};
pub use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::{
    compositor::CompositorState,
    delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{Capability, SeatHandler, SeatState},
    shell::wlr_layer::LayerShell,
};
pub use wayland_client::EventQueue;
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_seat},
};

/// Which outputs get an overlay surface (`overlay.outputs`). Every output gets a pointer.
#[derive(Clone, Debug, Default)]
pub enum OverlayOutputs {
    #[default]
    All,
    Named(Vec<String>),
}

impl OverlayOutputs {
    fn includes(&self, name: &str) -> bool {
        match self {
            Self::All => true,
            Self::Named(names) => names.iter().any(|n| n == name),
        }
    }
}

struct Output {
    // Field order is drop order: the overlay (and its GPU surface) before the output proxy.
    surface: Option<overlay::Surface>,
    pointer: input::OutputPointer,
    wl: wl_output::WlOutput,
    info: OutputInfo,
}

pub struct Wayland {
    // Outputs first: their GPU surfaces hold raw pointers into the connection below.
    outputs: Vec<Output>,
    pointers: input::Pointers,
    overlay_outputs: OverlayOutputs,
    next_id: u32,
    registry: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    qh: QueueHandle<Self>,
    conn: Connection,
}

fn wayland_error(err: impl std::fmt::Display) -> PlatformError {
    PlatformError::Wayland(err.to_string())
}

/// Connects to `$WAYLAND_DISPLAY`, binds what the overlay and the pointer need, and learns the
/// outputs and the seat.
pub fn connect(
    overlay_outputs: OverlayOutputs,
) -> Result<(Wayland, EventQueue<Wayland>), PlatformError> {
    let conn = Connection::connect_to_env().map_err(wayland_error)?;
    let (globals, mut queue) = registry_queue_init::<Wayland>(&conn).map_err(wayland_error)?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|_| PlatformError::Unsupported("wl_compositor"))?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .map_err(|_| PlatformError::Unsupported("zwlr_layer_shell_v1"))?;
    let pointers = input::Pointers::bind(&globals, &qh)?;
    let mut wayland = Wayland {
        outputs: Vec::new(),
        pointers,
        overlay_outputs,
        next_id: 1,
        registry: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        compositor,
        layer_shell,
        qh,
        conn,
    };
    // Outputs and seats announce themselves over the first two round trips.
    for _ in 0..2 {
        queue.roundtrip(&mut wayland).map_err(wayland_error)?;
    }
    if wayland.outputs.is_empty() {
        return Err(PlatformError::Wayland(
            "the compositor has no outputs".into(),
        ));
    }
    Ok((wayland, queue))
}

/// Sends queued requests, waits up to `timeout` for events and dispatches them. For callers
/// without a calloop loop (trace replay); the daemon uses a [`WaylandSource`] instead.
pub fn dispatch(
    queue: &mut EventQueue<Wayland>,
    wayland: &mut Wayland,
    timeout: Duration,
) -> Result<(), PlatformError> {
    queue.flush().map_err(wayland_error)?;
    if let Some(guard) = queue.prepare_read()
        && overlay::wait_readable(&guard.connection_fd(), timeout)
    {
        guard.read().map_err(wayland_error)?;
    }
    queue.dispatch_pending(wayland).map_err(wayland_error)?;
    Ok(())
}

impl Wayland {
    pub fn features(&self) -> Features {
        Features {
            pointer_absolute: true,
            overlay: true,
        }
    }

    /// Sends queued requests now (the calloop source also flushes before the loop sleeps).
    pub fn flush(&self) -> Result<(), PlatformError> {
        self.conn.flush().map_err(wayland_error)
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    fn output_info(&self, id: OutputId, wl: &wl_output::WlOutput) -> Option<OutputInfo> {
        let info = self.output_state.info(wl)?;
        let mode = info.modes.iter().find(|m| m.current).map(|m| m.dimensions);
        let (width, height) = info
            .logical_size
            .or_else(|| {
                mode.map(|(w, h)| (w / info.scale_factor.max(1), h / info.scale_factor.max(1)))
            })
            .unwrap_or((0, 0));
        let (x, y) = info.logical_position.unwrap_or(info.location);
        // wl_output only reports integer scales; a mode that is not an integer multiple of the
        // logical size means the compositor applies a fractional scale.
        let scale = match mode {
            Some((mode_w, _)) if width > 0 => f64::from(mode_w) / f64::from(width),
            _ => f64::from(info.scale_factor.max(1)),
        };
        Some(OutputInfo {
            id,
            name: info
                .name
                .clone()
                .unwrap_or_else(|| format!("output-{}", id.0)),
            logical_rect: Rect {
                x,
                y,
                width,
                height,
            },
            scale,
        })
    }
}

impl OutputHandler for Wayland {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, wl: wl_output::WlOutput) {
        let id = OutputId(self.next_id);
        self.next_id += 1;
        let Some(info) = self.output_info(id, &wl) else {
            return;
        };
        if info.scale.fract() != 0.0 {
            tracing::warn!(
                output = info.name,
                scale = info.scale,
                "fractional output scale is untested"
            );
        }
        tracing::info!(output = info.name, rect = ?info.logical_rect, "output added");
        self.outputs.push(Output {
            surface: None,
            pointer: input::OutputPointer::default(),
            wl,
            info,
        });
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, wl: wl_output::WlOutput) {
        let Some(index) = self.outputs.iter().position(|o| o.wl == wl) else {
            return;
        };
        if let Some(info) = self.output_info(self.outputs[index].info.id, &wl) {
            self.outputs[index].info = info;
        }
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, wl: wl_output::WlOutput) {
        if let Some(index) = self.outputs.iter().position(|o| o.wl == wl) {
            let output = self.outputs.remove(index);
            tracing::info!(output = output.info.name, "output removed");
        }
    }
}

impl SeatHandler for Wayland {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: Capability,
    ) {
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: Capability,
    ) {
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl ProvidesRegistryState for Wayland {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

delegate_registry!(Wayland);
smithay_client_toolkit::delegate_dispatch2!(Wayland);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_outputs_filter_by_name() {
        assert!(OverlayOutputs::All.includes("eDP-1"));
        let named = OverlayOutputs::Named(vec!["HDMI-A-1".into()]);
        assert!(named.includes("HDMI-A-1"));
        assert!(!named.includes("eDP-1"));
    }
}
