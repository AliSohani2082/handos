use handos_hands::{
    HandState, Quality, Role,
    features::{Finger, Pinch, hu_screen, palm_center, pinch},
};

use crate::{
    claims::{Claims, Resource},
    mapping::Mapping,
    phase::{Phase, PhaseMachine},
    trigger::{PINCH, Tap},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerCmd {
    Move { x: f32, y: f32, output: u32 },
    Button { btn: Button, pressed: bool },
    Axis { dy: f32 },
    AxisStop,
}

#[derive(Debug, Default)]
pub struct Update {
    pub commands: Vec<PointerCmd>,
    pub events: Vec<(Finger, Phase)>,
}

#[derive(Default)]
pub struct PressedSet(u8);

impl PressedSet {
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    fn set(&mut self, btn: Button, pressed: bool, commands: &mut Vec<PointerCmd>) {
        let mask = 1 << btn as u8;
        if (self.0 & mask != 0) != pressed {
            self.0 ^= mask;
            commands.push(PointerCmd::Button { btn, pressed });
        }
    }

    fn release_all(&mut self, commands: &mut Vec<PointerCmd>) {
        for btn in [Button::Left, Button::Right] {
            self.set(btn, false, commands);
        }
    }
}

const FINGERS: [Finger; 2] = [Finger::Index, Finger::Middle];

#[derive(Clone, Copy)]
enum Action {
    Pending { tap: Tap, begin: [f32; 2] },
    Drag,
    Scroll { previous_y: f32 },
}

struct Active {
    finger: Finger,
    action: Action,
}

pub struct Pointer {
    role: Role,
    scroll_gain: f32,
    track_id: Option<u32>,
    output: Option<u32>,
    phases: [PhaseMachine; 2],
    claims: Claims,
    active: Option<Active>,
    pressed: PressedSet,
}

impl Default for Pointer {
    fn default() -> Self {
        Self::new(Role::Right, 1.0)
    }
}

impl Pointer {
    pub fn new(role: Role, scroll_gain: f32) -> Self {
        Self {
            role,
            scroll_gain,
            track_id: None,
            output: None,
            phases: [PhaseMachine::default(); 2],
            claims: Claims::default(),
            active: None,
            pressed: PressedSet::default(),
        }
    }

    pub fn pressed(&self) -> &PressedSet {
        &self.pressed
    }

    /// Call on sleep, pause, output loss and shutdown before sending any other input command.
    pub fn cancel(&mut self) -> Update {
        let mut update = Update::default();
        self.cancel_into(&mut update);
        update
    }

    fn cancel_into(&mut self, update: &mut Update) {
        if let Some(active) = self.active.take() {
            update.events.push((active.finger, Phase::Cancelled));
            if matches!(active.action, Action::Scroll { .. }) {
                update.commands.push(PointerCmd::AxisStop);
            }
        }
        self.pressed.release_all(&mut update.commands);
        self.phases = [PhaseMachine::default(); 2];
        self.claims = Claims::default();
        self.track_id = None;
        self.output = None;
    }

    pub fn update(
        &mut self,
        hand: Option<&HandState>,
        mapping: Option<Mapping>,
        now_us: u64,
        enabled: bool,
    ) -> Update {
        let (Some(hand), Some(map)) = (
            hand.filter(|h| enabled && h.role == self.role && h.quality != Quality::Lost),
            mapping,
        ) else {
            return self.cancel();
        };
        let mut update = Update::default();
        if self.track_id != Some(hand.track_id) || self.output != Some(map.output()) {
            self.cancel_into(&mut update);
            self.track_id = Some(hand.track_id);
            self.output = Some(map.output());
        }
        let palm = palm_center(&hand.image);
        let position = map.point(palm, [hand.image[5][0], hand.image[5][1]]);
        if !position.into_iter().all(f32::is_finite) {
            self.cancel_into(&mut update);
            return update;
        }

        let mut move_pointer = hand.quality == Quality::Tracked
            && !self
                .active
                .as_ref()
                .is_some_and(|active| matches!(active.action, Action::Scroll { .. }));
        let samples = FINGERS.map(|finger| {
            pinch(hand, finger)
                .filter(|p| p.strength.is_finite() && p.point.into_iter().all(f32::is_finite))
        });
        let phases = std::array::from_fn::<_, 2, _>(|i| {
            self.phases[i].update(samples[i].map(|p| p.strength), hand.quality, PINCH)
        });

        // Finish the current claim before considering another pinch in the same frame.
        for i in [0, 1] {
            let finger = FINGERS[i];
            if let Some(phase @ (Phase::Ended | Phase::Cancelled)) = phases[i]
                && self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.finger == finger)
                && let Some(mut active) = self.active.take()
            {
                self.claims.release(i as u8);
                update.events.push((finger, phase));
                match &mut active.action {
                    Action::Pending { tap, .. } if phase == Phase::Ended => {
                        if let Some(p) = samples[i] {
                            tap.observe(p.point);
                        }
                        if tap.end(now_us).is_some() {
                            if move_pointer {
                                push_move(&mut update.commands, map, position);
                                move_pointer = false;
                            }
                            let btn = if finger == Finger::Index {
                                Button::Left
                            } else {
                                Button::Right
                            };
                            self.pressed.set(btn, true, &mut update.commands);
                            self.pressed.set(btn, false, &mut update.commands);
                            update.events.push((finger, Phase::Fired));
                        }
                    }
                    Action::Drag => {
                        // Drop where the hand is now, not where it was last frame. A cancel
                        // releases immediately: its position is not trustworthy.
                        if phase == Phase::Ended && move_pointer {
                            push_move(&mut update.commands, map, position);
                            move_pointer = false;
                        }
                        self.pressed.release_all(&mut update.commands);
                    }
                    Action::Scroll { .. } => update.commands.push(PointerCmd::AxisStop),
                    Action::Pending { .. } => {}
                }
            }
        }
        for i in [0, 1] {
            let finger = FINGERS[i];
            if phases[i] == Some(Phase::Began)
                && let Some(p) = samples[i]
                && self
                    .claims
                    .acquire(
                        i as u8,
                        0,
                        &[Resource::Pinch(self.role, finger), Resource::Pointer],
                    )
                    .is_some()
            {
                self.active = Some(Active {
                    finger,
                    action: Action::Pending {
                        tap: Tap::new(now_us, p.point, hu_screen(&hand.image)),
                        begin: position,
                    },
                });
                update.events.push((finger, Phase::Began));
            }
            if phases[i] == Some(Phase::Changed)
                && let Some(active) = self.active.as_mut().filter(|a| a.finger == finger)
                && let Some(sample) = samples[i]
            {
                update.events.push((finger, Phase::Changed));
                let scroll_y = |p: Pinch| map.point(palm, p.point)[1];
                match active.action {
                    Action::Pending { mut tap, begin } => {
                        tap.observe(sample.point);
                        if finger == Finger::Index && (tap.held(now_us) || tap.moved()) {
                            push_move(&mut update.commands, map, begin);
                            self.pressed.set(Button::Left, true, &mut update.commands);
                            active.action = Action::Drag;
                        } else if finger == Finger::Middle && tap.held(now_us) {
                            active.action = Action::Scroll {
                                previous_y: scroll_y(sample),
                            };
                            move_pointer = false;
                        } else {
                            active.action = Action::Pending { tap, begin };
                        }
                    }
                    Action::Scroll { previous_y } => {
                        let y = scroll_y(sample);
                        let dy = (y - previous_y) * self.scroll_gain;
                        if dy.is_finite() && dy != 0.0 {
                            update.commands.push(PointerCmd::Axis { dy });
                        }
                        active.action = Action::Scroll { previous_y: y };
                        move_pointer = false;
                    }
                    Action::Drag => {}
                }
            }
        }
        if move_pointer {
            push_move(&mut update.commands, map, position);
        }
        update
    }
}

fn push_move(commands: &mut Vec<PointerCmd>, map: Mapping, [x, y]: [f32; 2]) {
    commands.push(PointerCmd::Move {
        x,
        y,
        output: map.output(),
    });
}
