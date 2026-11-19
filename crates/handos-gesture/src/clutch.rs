use handos_hands::{Hands, Quality, Role, features::Pose};

use crate::trigger::{Hold, Primitive, all_score};

const WAKE_US: u64 = 500_000;
const SLEEP_US: u64 = 1_000_000;
const NO_HAND_US: u64 = 30_000_000;
const GRACE_US: u64 = 300_000;
const WAKE: &[Primitive] = &[
    Primitive::Pose(Pose::Open),
    Primitive::Facing { palm: true },
];
const SLEEP: &[Primitive] = &[Primitive::Pose(Pose::Fist)];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Sleep,
    Default,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeChanged(pub Mode);

pub struct Clutch {
    mode: Mode,
    holds: [Hold; 2],
    wake_needs_release: [bool; 2],
    last_tracked: u64,
    grace_until: u64,
}

impl Default for Clutch {
    fn default() -> Self {
        Self {
            mode: Mode::Sleep,
            holds: [Hold::default(), Hold::default()],
            wake_needs_release: [false; 2],
            last_tracked: 0,
            grace_until: 0,
        }
    }
}

impl Clutch {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn pointer_enabled(&self, now_us: u64) -> bool {
        self.mode == Mode::Default && now_us >= self.grace_until
    }

    pub fn set_mode(&mut self, mode: Mode, now_us: u64) -> Option<ModeChanged> {
        if mode == self.mode {
            return None;
        }
        self.mode = mode;
        self.holds = [Hold::default(), Hold::default()];
        self.last_tracked = now_us;
        self.grace_until = now_us.saturating_add(GRACE_US);
        Some(ModeChanged(mode))
    }

    pub fn toggle(&mut self, now_us: u64) -> Option<ModeChanged> {
        self.set_mode(
            if self.mode == Mode::Sleep {
                Mode::Default
            } else {
                Mode::Sleep
            },
            now_us,
        )
    }

    pub fn update(&mut self, hands: &Hands, now_us: u64) -> Option<ModeChanged> {
        let (terms, duration, next) = match self.mode {
            Mode::Sleep => (WAKE, WAKE_US, Mode::Default),
            Mode::Default => (SLEEP, SLEEP_US, Mode::Sleep),
        };
        let mut fired = false;
        for (i, role) in [Role::Left, Role::Right].into_iter().enumerate() {
            let hand = hands.get(role);
            if hand.is_some_and(|h| h.quality == Quality::Tracked) {
                self.last_tracked = now_us;
            }
            let wake_pose = hand
                .and_then(|h| all_score(h, WAKE))
                .is_some_and(|score| score > 0.0);
            if !hand.is_some_and(|h| h.quality == Quality::Coasting) {
                if self.mode == Mode::Default {
                    // An open palm already visible at sleep must be lowered before it can wake again.
                    self.wake_needs_release[i] = wake_pose;
                } else if !wake_pose {
                    self.wake_needs_release[i] = false;
                }
            }
            if self.mode == Mode::Default || !self.wake_needs_release[i] {
                fired |= self.holds[i].update(hand, terms, now_us, duration);
            }
        }
        if fired
            || (self.mode == Mode::Default
                && now_us.saturating_sub(self.last_tracked) >= NO_HAND_US)
        {
            return self.set_mode(next, now_us);
        }
        None
    }
}
