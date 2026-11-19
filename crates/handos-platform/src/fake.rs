//! Backends that record every call, for tests that run the daemon without a desktop.

use crate::{Button, DisplayList, Input, OutputId, OutputInfo, Overlay, PlatformError};

#[derive(Clone, Debug, PartialEq)]
pub enum OverlayCall {
    Draw(OutputId, DisplayList),
    Clear(OutputId),
}

#[derive(Debug, Default)]
pub struct FakeOverlay {
    pub outputs: Vec<OutputInfo>,
    pub calls: Vec<OverlayCall>,
}

impl FakeOverlay {
    pub fn new(outputs: Vec<OutputInfo>) -> Self {
        Self {
            outputs,
            calls: Vec::new(),
        }
    }

    fn check(&self, output: OutputId) -> Result<(), PlatformError> {
        if self.outputs.iter().any(|o| o.id == output) {
            Ok(())
        } else {
            Err(PlatformError::UnknownOutput(output))
        }
    }
}

impl Overlay for FakeOverlay {
    fn outputs(&self) -> Vec<OutputInfo> {
        self.outputs.clone()
    }

    fn draw(&mut self, output: OutputId, list: &DisplayList) -> Result<(), PlatformError> {
        self.check(output)?;
        self.calls.push(OverlayCall::Draw(output, list.clone()));
        Ok(())
    }

    fn clear(&mut self, output: OutputId) {
        // Like a real backend, clearing a disconnected output is a no-op rather than an error.
        if self.check(output).is_ok() {
            self.calls.push(OverlayCall::Clear(output));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputCall {
    PointerAbs(OutputId, f64, f64),
    Button(Button, bool),
    Axis(f64),
    AxisStop,
}

#[derive(Debug, Default)]
pub struct FakeInput {
    pub calls: Vec<InputCall>,
}

impl FakeInput {
    /// Buttons whose last recorded event was a press.
    pub fn held(&self) -> Vec<Button> {
        let mut held = Vec::new();
        for call in &self.calls {
            if let InputCall::Button(btn, pressed) = *call {
                held.retain(|b| *b != btn);
                if pressed {
                    held.push(btn);
                }
            }
        }
        held
    }
}

impl Input for FakeInput {
    fn pointer_abs(&mut self, output: OutputId, x: f64, y: f64) {
        self.calls.push(InputCall::PointerAbs(output, x, y));
    }

    fn button(&mut self, btn: Button, pressed: bool) {
        self.calls.push(InputCall::Button(btn, pressed));
    }

    fn axis(&mut self, dy: f64) {
        self.calls.push(InputCall::Axis(dy));
    }

    fn axis_stop(&mut self) {
        self.calls.push(InputCall::AxisStop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Prim, Rect};

    fn output(id: u32) -> OutputInfo {
        OutputInfo {
            id: OutputId(id),
            name: format!("DP-{id}"),
            logical_rect: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            scale: 1.0,
        }
    }

    #[test]
    fn overlay_records_draws_and_clears_on_known_outputs() {
        let mut overlay = FakeOverlay::new(vec![output(1)]);
        let list = DisplayList(vec![Prim::Dot {
            at: [10.0, 20.0],
            radius: 4.0,
            rgba: [0.0, 1.0, 0.0, 1.0],
        }]);
        overlay.draw(OutputId(1), &list).unwrap();
        overlay.clear(OutputId(1));
        assert_eq!(
            overlay.calls,
            vec![
                OverlayCall::Draw(OutputId(1), list),
                OverlayCall::Clear(OutputId(1))
            ]
        );
    }

    #[test]
    fn overlay_rejects_draws_on_unknown_outputs() {
        let mut overlay = FakeOverlay::new(vec![output(1)]);
        let err = overlay
            .draw(OutputId(2), &DisplayList::default())
            .unwrap_err();
        assert!(matches!(err, PlatformError::UnknownOutput(OutputId(2))));
        overlay.clear(OutputId(2));
        assert!(overlay.calls.is_empty());
    }

    #[test]
    fn input_tracks_held_buttons() {
        let mut input = FakeInput::default();
        input.button(Button::Left, true);
        input.button(Button::Right, true);
        input.button(Button::Left, false);
        assert_eq!(input.held(), vec![Button::Right]);
        input.button(Button::Right, false);
        assert!(input.held().is_empty());
    }

    #[test]
    fn input_records_calls_in_order() {
        let mut input = FakeInput::default();
        input.pointer_abs(OutputId(1), 5.0, 6.0);
        input.axis(-3.0);
        input.axis_stop();
        assert_eq!(
            input.calls,
            vec![
                InputCall::PointerAbs(OutputId(1), 5.0, 6.0),
                InputCall::Axis(-3.0),
                InputCall::AxisStop
            ]
        );
    }
}
