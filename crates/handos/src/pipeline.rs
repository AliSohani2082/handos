//! One HandFrame through M1: condition → clutch → pointer, then input and overlay (Task C7).
//!
//! Pure logic over the `Overlay`/`Input` traits, so the daemon, trace replay and tests share it.

use handos_gesture::{
    clutch::{Clutch, Mode},
    mapping::Mapping,
    pointer::{self, Pointer, PointerCmd},
};
use handos_hands::{
    Conditioner, HandState, Quality, Role,
    features::{hu_screen, palm_center},
    predict::predict,
};
use handos_platform::{
    Button, Desktop, DisplayList, Input, OutputId, OutputInfo, Overlay, PlatformError, Prim,
};
use handos_proto::HandFrame;
use handos_render::{Style, skeleton};

/// The hand box (palm centre, mirrored image coordinates) until settings (D1) and calibration
/// (D7) supply one: a palm centre rarely gets closer than this to the frame edges.
pub const DEFAULT_BOX: [[f32; 2]; 2] = [[0.15, 0.15], [0.85, 0.85]];
/// Skeletons are drawn where the hand will be when the frame reaches the screen.
const PREDICT_US: u64 = 8_000;
/// Without a hand for this long, the tracker drops to its idle rate.
const IDLE_AFTER_US: u64 = 3_000_000;
/// Faster palms keep the tracker at full rate, in hand units per second.
const FAST_HU_PER_S: f32 = 0.5;
const PALM: [usize; 5] = [0, 5, 9, 13, 17];
const MODE_DOT: [f32; 4] = [0.2, 0.9, 0.4, 0.95];

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub pointer_hand: Role,
    pub scroll_gain: f32,
    pub mapping_box: [[f32; 2]; 2],
    pub hand_scale: f32,
    pub style: Style,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pointer_hand: Role::Right,
            scroll_gain: 1.0,
            mapping_box: DEFAULT_BOX,
            hand_scale: 0.6,
            style: Style::default(),
        }
    }
}

/// The tracker rate the pipeline wants (the daemon turns it into a `set` message).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rate {
    Active { fps: u8 },
    Idle,
}

#[derive(Debug)]
pub struct Step {
    pub mode_changed: Option<Mode>,
    pub rate: Rate,
}

pub struct Pipeline {
    config: Config,
    conditioner: Conditioner,
    clutch: Clutch,
    pointer: Pointer,
    target: Option<OutputInfo>,
    drawn: Vec<OutputId>,
    last_hand_us: Option<u64>,
    warned_draw: bool,
}

impl Pipeline {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            conditioner: Conditioner::default(),
            clutch: Clutch::default(),
            pointer: Pointer::new(config.pointer_hand, config.scroll_gain),
            target: None,
            drawn: Vec::new(),
            last_hand_us: None,
            warned_draw: false,
        }
    }

    pub fn mode(&self) -> Mode {
        self.clutch.mode()
    }

    /// Wakes or sleeps from outside the gesture (trace replay `--awake`; later the control socket).
    pub fn set_mode(&mut self, mode: Mode, now_us: u64) -> Option<Mode> {
        self.clutch.set_mode(mode, now_us).map(|changed| changed.0)
    }

    /// The output the pointer maps to: the focused one at wake time, else the first.
    pub fn target(&self) -> Option<&OutputInfo> {
        self.target.as_ref()
    }

    pub fn on_frame(
        &mut self,
        frame: &HandFrame,
        now_us: u64,
        desktop: &mut dyn Desktop,
        focused: &dyn Fn() -> Option<String>,
    ) -> Step {
        let hands = self.conditioner.update(frame, now_us);
        let mode_changed = self.clutch.update(hands, now_us).map(|changed| changed.0);
        let awake = self.clutch.mode() == Mode::Default;
        if awake {
            let woke = mode_changed == Some(Mode::Default);
            let before = self.target.as_ref().map(|t| t.id);
            self.target = select_target(self.target.take(), &desktop.outputs(), woke, focused);
            if let Some(target) = &self.target
                && (woke || before != Some(target.id))
            {
                tracing::info!(output = target.name, "pointer output");
            }
        }
        let mapping = self
            .target
            .as_ref()
            .and_then(|target| mapping_for(&self.config, target));
        let enabled = self.clutch.pointer_enabled(now_us);
        let update = self.pointer.update(
            hands.get(self.config.pointer_hand),
            mapping,
            now_us,
            enabled,
        );
        send(&update.commands, desktop);

        let visible: Vec<&HandState> = [hands.left.as_ref(), hands.right.as_ref()]
            .into_iter()
            .flatten()
            .filter(|hand| hand.quality != Quality::Lost)
            .collect();
        let started = *self.last_hand_us.get_or_insert(now_us);
        if !visible.is_empty() {
            self.last_hand_us = Some(now_us);
        }

        let shown = match (awake, &self.target, mapping) {
            (true, Some(target), Some(mapping)) if !visible.is_empty() => {
                let list = display_list(&visible, mapping, target, self.config.style, now_us);
                match desktop.draw(target.id, &list) {
                    Ok(()) => Some(target.id),
                    Err(err) => {
                        warn_once(&mut self.warned_draw, &err);
                        None
                    }
                }
            }
            _ => None,
        };
        for id in self.drawn.drain(..).filter(|&id| Some(id) != shown) {
            desktop.clear(id);
        }
        self.drawn.extend(shown);

        let idle = now_us.saturating_sub(self.last_hand_us.unwrap_or(started)) >= IDLE_AFTER_US;
        let fast =
            self.pointer.is_active() || visible.iter().any(|h| palm_speed(h) > FAST_HU_PER_S);
        Step {
            mode_changed,
            rate: rate(idle, fast),
        }
    }

    /// Releases every pressed button. Call on every shutdown path before input goes away.
    pub fn release(&mut self, input: &mut dyn Input) {
        let update = self.pointer.cancel();
        send(&update.commands, input);
    }

    /// Releases buttons, then clears the overlay: for tracker loss and shutdown, when no more
    /// frames will come to do it.
    pub fn suspend(&mut self, desktop: &mut dyn Desktop) {
        self.release(desktop);
        for id in self.drawn.drain(..) {
            desktop.clear(id);
        }
    }
}

fn warn_once(warned: &mut bool, err: &PlatformError) {
    if !*warned {
        *warned = true;
        tracing::warn!(%err, "overlay draw failed; drawing continues on the next frame");
    }
}

/// The output the pointer maps to this frame. The current one is kept, with its geometry
/// refreshed (a mode or scale change must reach the mapping); the focused output is chosen again
/// only on wake or when the current one has gone.
fn select_target(
    current: Option<OutputInfo>,
    outputs: &[OutputInfo],
    woke: bool,
    focused: &dyn Fn() -> Option<String>,
) -> Option<OutputInfo> {
    let refreshed = current.and_then(|t| outputs.iter().find(|o| o.id == t.id).cloned());
    match refreshed {
        Some(target) if !woke => Some(target),
        _ => choose_target(outputs, focused().as_deref()),
    }
}

fn choose_target(outputs: &[OutputInfo], focused: Option<&str>) -> Option<OutputInfo> {
    outputs
        .iter()
        .find(|o| Some(o.name.as_str()) == focused)
        .or_else(|| outputs.first())
        .cloned()
}

fn mapping_for(config: &Config, target: &OutputInfo) -> Option<Mapping> {
    let rect = target.logical_rect;
    Mapping::new(
        config.mapping_box[0],
        config.mapping_box[1],
        config.hand_scale,
        target.id.0,
        [0.0; 2],
        [rect.width as f32, rect.height as f32],
    )
}

fn rate(idle: bool, fast: bool) -> Rate {
    match (idle, fast) {
        (true, _) => Rate::Idle,
        (false, true) => Rate::Active { fps: 30 },
        (false, false) => Rate::Active { fps: 15 },
    }
}

/// Palm speed in hand units per second, from the filter's velocity.
fn palm_speed(hand: &HandState) -> f32 {
    let unit = hu_screen(&hand.image);
    if unit <= 1e-6 {
        return 0.0;
    }
    let [vx, vy] = PALM.iter().fold([0.0, 0.0], |[x, y], &i| {
        [x + hand.velocity[i][0], y + hand.velocity[i][1]]
    });
    let n = PALM.len() as f32;
    (vx / n).hypot(vy / n) / unit
}

fn display_list(
    hands: &[&HandState],
    mapping: Mapping,
    target: &OutputInfo,
    style: Style,
    now_us: u64,
) -> DisplayList {
    let mut list = DisplayList::default();
    for hand in hands {
        let points = predict(hand, now_us + PREDICT_US);
        let palm = palm_center(&points);
        let screen: [[f32; 2]; 21] =
            std::array::from_fn(|i| mapping.point(palm, [points[i][0], points[i][1]]));
        list.extend(skeleton(&screen, style));
    }
    // The mode badge: a coloured dot, not text (NOT Building: text is M3).
    list.0.push(Prim::Dot {
        at: [target.logical_rect.width as f32 - 24.0, 24.0],
        radius: 7.0,
        rgba: MODE_DOT,
    });
    list
}

fn send(commands: &[PointerCmd], input: &mut dyn Input) {
    for cmd in commands {
        match *cmd {
            PointerCmd::Move { x, y, output } => {
                input.pointer_abs(OutputId(output), f64::from(x), f64::from(y));
            }
            PointerCmd::Button { btn, pressed } => input.button(button(btn), pressed),
            // The gesture reports the hand's motion (screen y grows downwards) and scrolling is
            // natural, so content follows the hand; the platform's positive moves content up.
            PointerCmd::Axis { dy } => input.axis(-f64::from(dy)),
            PointerCmd::AxisStop => input.axis_stop(),
        }
    }
}

fn button(btn: pointer::Button) -> Button {
    match btn {
        pointer::Button::Left => Button::Left,
        pointer::Button::Right => Button::Right,
    }
}

/// A desktop with no outputs, for `--headless` runs: nothing is drawn or sent.
#[derive(Debug, Default)]
pub struct Headless;

impl Overlay for Headless {
    fn outputs(&self) -> Vec<OutputInfo> {
        Vec::new()
    }

    fn draw(&mut self, output: OutputId, _: &DisplayList) -> Result<(), PlatformError> {
        Err(PlatformError::UnknownOutput(output))
    }

    fn clear(&mut self, _: OutputId) {}
}

impl Input for Headless {
    fn pointer_abs(&mut self, _: OutputId, _: f64, _: f64) {}
    fn button(&mut self, _: Button, _: bool) {}
    fn axis(&mut self, _: f64) {}
    fn axis_stop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use handos_platform::{
        Rect,
        fake::{FakeInput, InputCall},
    };

    use super::*;

    fn output(id: u32, name: &str) -> OutputInfo {
        OutputInfo {
            id: OutputId(id),
            name: name.into(),
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
    fn the_focused_output_wins_then_the_first() {
        let outputs = [output(1, "eDP-1"), output(2, "HDMI-A-1")];
        assert_eq!(
            choose_target(&outputs, Some("HDMI-A-1")).unwrap().id,
            OutputId(2)
        );
        assert_eq!(
            choose_target(&outputs, Some("DP-9")).unwrap().id,
            OutputId(1)
        );
        assert_eq!(choose_target(&outputs, None).unwrap().id, OutputId(1));
        assert!(choose_target(&[], None).is_none());
    }

    #[test]
    fn the_target_keeps_its_id_but_takes_new_geometry() {
        let focused = || Some("HDMI-A-1".to_owned());
        let mut outputs = vec![output(1, "eDP-1"), output(2, "HDMI-A-1")];
        let target = select_target(None, &outputs, true, &focused);
        assert_eq!(target.as_ref().map(|t| t.id), Some(OutputId(2)));
        // The monitor changes mode while awake; focus moves elsewhere but that is not a wake.
        outputs[1].logical_rect.width = 2560;
        let target = select_target(target, &outputs, false, &|| Some("eDP-1".to_owned()));
        assert_eq!(
            target.as_ref().map(|t| (t.id, t.logical_rect.width)),
            Some((OutputId(2), 2560))
        );
        // Unplugged: fall back to the focused output.
        outputs.remove(1);
        let target = select_target(target, &outputs, false, &|| Some("eDP-1".to_owned()));
        assert_eq!(target.map(|t| t.id), Some(OutputId(1)));
    }

    #[test]
    fn commands_reach_input_with_natural_scrolling() {
        let mut input = FakeInput::default();
        send(
            &[
                PointerCmd::Move {
                    x: 10.0,
                    y: 20.0,
                    output: 2,
                },
                PointerCmd::Button {
                    btn: pointer::Button::Right,
                    pressed: true,
                },
                // The hand moved down 5 px: the content follows it down, i.e. scrolls up.
                PointerCmd::Axis { dy: 5.0 },
                PointerCmd::AxisStop,
            ],
            &mut input,
        );
        assert_eq!(
            input.calls,
            [
                InputCall::PointerAbs(OutputId(2), 10.0, 20.0),
                InputCall::Button(Button::Right, true),
                InputCall::Axis(-5.0),
                InputCall::AxisStop,
            ]
        );
    }

    #[test]
    fn rate_is_idle_without_hands_then_fast_while_moving() {
        assert_eq!(rate(true, true), Rate::Idle);
        assert_eq!(rate(false, true), Rate::Active { fps: 30 });
        assert_eq!(rate(false, false), Rate::Active { fps: 15 });
    }

    #[test]
    fn the_mapping_covers_the_target_output() {
        let map = mapping_for(&Config::default(), &output(3, "DP-1")).unwrap();
        assert_eq!(map.output(), 3);
        let [x0, y0] = DEFAULT_BOX[0];
        assert_eq!(map.point([x0, y0], [x0, y0]), [0.0, 0.0]);
        let [x1, y1] = DEFAULT_BOX[1];
        assert_eq!(map.point([x1, y1], [x1, y1]), [1920.0, 1080.0]);
    }
}
