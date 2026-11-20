//! A wlr virtual pointer per output: absolute motion in that output's logical pixels.

use std::time::Instant;

use handos_platform::{Button, Input, OutputId, PlatformError};
use smithay_client_toolkit::dispatch2::Dispatch2;
use wayland_client::{
    Connection, QueueHandle,
    globals::{BindError, GlobalList},
    protocol::wl_pointer,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::{self, ZwlrVirtualPointerManagerV1},
    zwlr_virtual_pointer_v1::{self, ZwlrVirtualPointerV1},
};

use crate::Wayland;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

pub(crate) struct Pointers {
    manager: ZwlrVirtualPointerManagerV1,
    start: Instant,
    /// The output the pointer last moved on; buttons and scrolling go to its device.
    last: Option<OutputId>,
    warned_no_seat: bool,
}

#[derive(Default)]
pub(crate) struct OutputPointer(Option<ZwlrVirtualPointerV1>);

impl Drop for OutputPointer {
    fn drop(&mut self) {
        if let Some(pointer) = self.0.take() {
            pointer.destroy();
        }
    }
}

impl Pointers {
    /// Version 2 is required: version 1 maps absolute motion over the whole layout instead of
    /// one output, which breaks the per-output mapping.
    pub(crate) fn bind(
        globals: &GlobalList,
        qh: &QueueHandle<Wayland>,
    ) -> Result<Self, PlatformError> {
        let manager = globals.bind(qh, 2..=2, NoEvents).map_err(|err| match err {
            BindError::UnsupportedVersion => PlatformError::Unsupported(
                "zwlr_virtual_pointer_manager_v1 version 2 (only version 1 is offered)",
            ),
            BindError::NotPresent => PlatformError::Unsupported("zwlr_virtual_pointer_manager_v1"),
        })?;
        Ok(Self {
            manager,
            start: Instant::now(),
            last: None,
            warned_no_seat: false,
        })
    }

    fn now_ms(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }
}

impl Wayland {
    /// The device for `output` (created on first use), or `None` without a seat or output.
    fn pointer_on(&mut self, output: OutputId) -> Option<ZwlrVirtualPointerV1> {
        let seat = self.seat_state.seats().next();
        if seat.is_none() && !self.pointers.warned_no_seat {
            self.pointers.warned_no_seat = true;
            tracing::warn!("no seat yet: pointer events are dropped");
        }
        let seat = seat?;
        let out = self.outputs.iter_mut().find(|o| o.info.id == output)?;
        let pointer = out.pointer.0.get_or_insert_with(|| {
            self.pointers.manager.create_virtual_pointer_with_output(
                Some(&seat),
                Some(&out.wl),
                &self.qh,
                NoEvents,
            )
        });
        Some(pointer.clone())
    }

    /// The device that last moved, so a click lands where the pointer is.
    fn current_pointer(&mut self) -> Option<ZwlrVirtualPointerV1> {
        let output = self
            .pointers
            .last
            .or_else(|| self.outputs.first().map(|o| o.info.id))?;
        self.pointer_on(output)
    }
}

impl Input for Wayland {
    fn pointer_abs(&mut self, output: OutputId, x: f64, y: f64) {
        let Some(rect) = self
            .outputs
            .iter()
            .find(|o| o.info.id == output)
            .map(|o| o.info.logical_rect)
        else {
            return;
        };
        let (w, h) = (rect.width.max(1) as u32, rect.height.max(1) as u32);
        let Some(pointer) = self.pointer_on(output) else {
            return;
        };
        let x = x.clamp(0.0, f64::from(w - 1)) as u32;
        let y = y.clamp(0.0, f64::from(h - 1)) as u32;
        pointer.motion_absolute(self.pointers.now_ms(), x, y, w, h);
        pointer.frame();
        self.pointers.last = Some(output);
    }

    fn button(&mut self, btn: Button, pressed: bool) {
        let Some(pointer) = self.current_pointer() else {
            return;
        };
        let code = match btn {
            Button::Left => BTN_LEFT,
            Button::Right => BTN_RIGHT,
        };
        let state = if pressed {
            wl_pointer::ButtonState::Pressed
        } else {
            wl_pointer::ButtonState::Released
        };
        pointer.button(self.pointers.now_ms(), code, state);
        pointer.frame();
    }

    fn axis(&mut self, dy: f64) {
        let Some(pointer) = self.current_pointer() else {
            return;
        };
        pointer.axis_source(wl_pointer::AxisSource::Finger);
        // Wayland's sign: positive scrolls down, which moves the content up.
        pointer.axis(self.pointers.now_ms(), wl_pointer::Axis::VerticalScroll, dy);
        pointer.frame();
    }

    fn axis_stop(&mut self) {
        let Some(pointer) = self.current_pointer() else {
            return;
        };
        pointer.axis_source(wl_pointer::AxisSource::Finger);
        pointer.axis_stop(self.pointers.now_ms(), wl_pointer::Axis::VerticalScroll);
        pointer.frame();
    }
}

/// The virtual pointer objects send no events.
pub(crate) struct NoEvents;

impl Dispatch2<ZwlrVirtualPointerManagerV1, Wayland> for NoEvents {
    fn event(
        &self,
        _: &mut Wayland,
        _: &ZwlrVirtualPointerManagerV1,
        _: zwlr_virtual_pointer_manager_v1::Event,
        _: &Connection,
        _: &QueueHandle<Wayland>,
    ) {
    }
}

impl Dispatch2<ZwlrVirtualPointerV1, Wayland> for NoEvents {
    fn event(
        &self,
        _: &mut Wayland,
        _: &ZwlrVirtualPointerV1,
        _: zwlr_virtual_pointer_v1::Event,
        _: &Connection,
        _: &QueueHandle<Wayland>,
    ) {
    }
}
