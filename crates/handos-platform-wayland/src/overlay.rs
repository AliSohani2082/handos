//! A click-through layer surface per output, drawn with its own `Gpu` (created lazily).
#![allow(unsafe_code)]

use std::{os::fd::AsRawFd, ptr::NonNull, time::Duration};

use handos_platform::{DisplayList, OutputId, OutputInfo, Overlay, PlatformError};
use handos_render::{Gpu, Presented};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
};
use wayland_client::{
    Connection, Proxy, QueueHandle,
    protocol::{wl_output, wl_surface},
};

use crate::Wayland;

pub(crate) struct Surface {
    // Field order is drop order: the wgpu surface must go before the wl_surface it draws to.
    gpu: Option<Gpu>,
    layer: LayerSurface,
    /// Logical size from the last configure; (0, 0) until the first one.
    size: (u32, u32),
    scale: i32,
    /// A frame callback is outstanding: the compositor has not shown the last frame yet.
    frame_pending: bool,
    /// The newest list that arrived while waiting for a configure or a frame callback.
    pending: Option<DisplayList>,
    /// The last committed frame had something on it, so a clear has work to do.
    showing: bool,
    /// GPU setup failed once; drawing on this output is off until it is recreated.
    failed: bool,
}

impl Surface {
    fn create(
        compositor: &CompositorState,
        layer_shell: &LayerShell,
        qh: &QueueHandle<Wayland>,
        output: &wl_output::WlOutput,
        scale: i32,
    ) -> Self {
        let wl_surface = compositor.create_surface(qh);
        wl_surface.set_buffer_scale(scale);
        let layer = layer_shell.create_layer_surface(
            qh,
            wl_surface,
            Layer::Overlay,
            Some("handos"),
            Some(output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(0, 0);
        // An empty input region: every click passes through to the windows below.
        match Region::new(compositor) {
            Ok(region) => layer.set_input_region(Some(region.wl_region())),
            Err(err) => tracing::warn!(%err, "no empty input region: the overlay may take clicks"),
        }
        layer.commit();
        Self {
            gpu: None,
            layer,
            size: (0, 0),
            scale,
            frame_pending: false,
            pending: None,
            showing: false,
            failed: false,
        }
    }

    fn pixels(&self) -> (u32, u32) {
        let scale = self.scale.max(1) as u32;
        (self.size.0 * scale, self.size.1 * scale)
    }

    fn resize(&mut self) {
        let (w, h) = self.pixels();
        let scale = self.scale.max(1) as f32;
        if let Some(gpu) = &mut self.gpu {
            gpu.resize(w, h, scale);
        }
    }

    /// Draws `list` now, creating the GPU on first use. Content asks for a frame callback (so the
    /// next list waits for the compositor); an empty list does not, since Hyprland sends no
    /// callback for an unchanged surface and a clear must never wait for one.
    /// Returns whether the frame was committed.
    fn present(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Wayland>,
        list: &DisplayList,
    ) -> Result<bool, PlatformError> {
        if self.failed {
            return Ok(false);
        }
        if self.gpu.is_none() {
            let display = NonNull::new(conn.backend().display_ptr().cast());
            let surface = NonNull::new(self.layer.wl_surface().id().as_ptr().cast());
            let (Some(display), Some(surface)) = (display, surface) else {
                self.failed = true;
                return Err(PlatformError::Gpu(
                    "no Wayland display or surface pointer".into(),
                ));
            };
            // SAFETY: the connection lives in `Wayland` and outlives every output, and the
            // wl_surface belongs to `self.layer`, which `self.gpu` is declared before and so
            // dropped before.
            match unsafe { Gpu::new(display, surface) } {
                Ok(gpu) => {
                    tracing::info!(adapter = gpu.adapter(), "overlay GPU ready");
                    self.gpu = Some(gpu);
                    self.resize();
                }
                Err(err) => {
                    self.failed = true;
                    return Err(PlatformError::Gpu(err.to_string()));
                }
            }
        }
        let wl_surface = self.layer.wl_surface();
        if !list.is_empty() {
            // Must be requested before `present()` commits.
            wl_surface.frame(qh, FrameCallbackData(wl_surface.clone()));
        }
        let Some(gpu) = &mut self.gpu else {
            return Ok(false);
        };
        match gpu.render(list) {
            Presented::Done => {
                self.frame_pending = !list.is_empty();
                self.showing = !list.is_empty();
                self.pending = None;
                Ok(true)
            }
            Presented::Skipped => {
                // No texture this time (resize in flight): keep the list for the next chance.
                self.pending = Some(list.clone());
                Ok(false)
            }
            Presented::Lost => {
                // Recreated on the next present, on the same live wl_surface.
                tracing::warn!("overlay GPU surface lost; recreating it");
                self.gpu = None;
                self.pending = Some(list.clone());
                Ok(false)
            }
        }
    }
}

impl Overlay for Wayland {
    fn outputs(&self) -> Vec<OutputInfo> {
        self.outputs.iter().map(|o| o.info.clone()).collect()
    }

    fn draw(&mut self, output: OutputId, list: &DisplayList) -> Result<(), PlatformError> {
        let Some(out) = self.outputs.iter_mut().find(|o| o.info.id == output) else {
            return Err(PlatformError::UnknownOutput(output));
        };
        if !self.overlay_outputs.includes(&out.info.name) {
            return Ok(());
        }
        let scale = out.info.scale.round().max(1.0) as i32;
        let surface = out.surface.get_or_insert_with(|| {
            Surface::create(
                &self.compositor,
                &self.layer_shell,
                &self.qh,
                &out.wl,
                scale,
            )
        });
        if surface.size == (0, 0) || surface.frame_pending {
            surface.pending = Some(list.clone());
            return Ok(());
        }
        surface.present(&self.conn, &self.qh, list).map(drop)
    }

    fn clear(&mut self, output: OutputId) {
        let Some(surface) = self
            .outputs
            .iter_mut()
            .find(|o| o.info.id == output)
            .and_then(|o| o.surface.as_mut())
        else {
            return;
        };
        surface.pending = None;
        if !surface.showing {
            return;
        }
        surface.frame_pending = false;
        match surface.present(&self.conn, &self.qh, &DisplayList::default()) {
            Ok(true) => {}
            outcome => {
                // The skeleton would stay on screen until the next draw, and after a suspend no
                // draw comes: destroy the surface instead (recreated on the next draw).
                if let Err(err) = outcome {
                    tracing::warn!(%err, "could not clear the overlay");
                }
                if let Some(out) = self.outputs.iter_mut().find(|o| o.info.id == output) {
                    out.surface = None;
                }
            }
        }
    }
}

impl Wayland {
    fn surface_for(&mut self, wl_surface: &wl_surface::WlSurface) -> Option<&mut Surface> {
        self.outputs
            .iter_mut()
            .filter_map(|o| o.surface.as_mut())
            .find(|s| s.layer.wl_surface() == wl_surface)
    }
}

impl CompositorHandler for Wayland {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        wl_surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        if let Some(surface) = self.surface_for(wl_surface) {
            surface.scale = factor.max(1);
            wl_surface.set_buffer_scale(surface.scale);
            surface.resize();
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        wl_surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        let (conn, qh) = (self.conn.clone(), self.qh.clone());
        let Some(surface) = self.surface_for(wl_surface) else {
            return;
        };
        surface.frame_pending = false;
        if let Some(list) = surface.pending.take()
            && let Err(err) = surface.present(&conn, &qh, &list)
        {
            tracing::warn!(%err, "overlay draw failed");
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for Wayland {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        // The compositor took the surface away (output gone, session locked): recreate on demand.
        for output in &mut self.outputs {
            if output.surface.as_ref().is_some_and(|s| &s.layer == layer) {
                output.surface = None;
            }
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        cfg: LayerSurfaceConfigure,
        _: u32,
    ) {
        let (conn, qh) = (self.conn.clone(), self.qh.clone());
        let Some(surface) = self.surface_for(layer.wl_surface()) else {
            return;
        };
        surface.size = cfg.new_size;
        surface.resize();
        if !surface.frame_pending
            && let Some(list) = surface.pending.take()
            && let Err(err) = surface.present(&conn, &qh, &list)
        {
            tracing::warn!(%err, "overlay draw failed");
        }
    }
}

/// Waits until `fd` is readable or `timeout` passes.
pub(crate) fn wait_readable(fd: &impl AsRawFd, timeout: Duration) -> bool {
    let mut pfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // Round up: a 0 ms poll for a sub-millisecond wait would spin.
    let ms = i32::try_from(timeout.as_micros().div_ceil(1000)).unwrap_or(i32::MAX);
    // SAFETY: one valid pollfd that lives for the duration of the call.
    unsafe { libc::poll(&mut pfd, 1, ms) > 0 }
}
