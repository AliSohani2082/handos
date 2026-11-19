use handos_hands::{
    HandState, Quality,
    features::{Finger, Pose, facing, pinch, pose_score},
};

use crate::phase::{Phase, PhaseMachine, Thresholds};

pub const PINCH: Thresholds = Thresholds {
    enter: 0.75,
    exit: 0.45,
};
const POSE: Thresholds = Thresholds {
    enter: 0.7,
    exit: 0.5,
};
const FACING: Thresholds = Thresholds {
    enter: 0.75,
    exit: 0.6,
};
pub const TAP_US: u64 = 300_000;
pub const TAP_MOVE_HU: f32 = 0.3;

#[derive(Clone, Copy)]
pub enum Primitive {
    Pinch(Finger),
    Pose(Pose),
    Facing { palm: bool },
    Present,
}

impl Primitive {
    fn sample(self, hand: &HandState) -> Option<(f32, Thresholds)> {
        Some(match self {
            Self::Pinch(finger) => (pinch(hand, finger)?.strength, PINCH),
            Self::Pose(pose) => (pose_score(hand, pose)?, POSE),
            Self::Facing { palm } => {
                let score = facing(hand)?;
                (if palm { 1.0 - score } else { score }, FACING)
            }
            Self::Present => (
                1.0,
                Thresholds {
                    enter: 1.0,
                    exit: 0.0,
                },
            ),
        })
    }
}

/// Evaluate an AND on one hand so two different hands cannot supply its conditions.
#[derive(Default)]
pub struct All {
    phase: PhaseMachine,
    track_id: Option<u32>,
}

pub(crate) fn all_score(hand: &HandState, terms: &[Primitive]) -> Option<f32> {
    if terms.is_empty() {
        return None;
    }
    let mut above_enter = true;
    let mut above_exit = true;
    for term in terms {
        let (score, limits) = term.sample(hand)?;
        above_enter &= score >= limits.enter;
        above_exit &= score > limits.exit;
    }
    Some(if above_enter {
        1.0
    } else if above_exit {
        0.5
    } else {
        0.0
    })
}

impl All {
    pub fn update(&mut self, hand: Option<&HandState>, terms: &[Primitive]) -> Option<Phase> {
        let id = hand.map(|hand| hand.track_id);
        if self.track_id != id {
            self.track_id = id;
            if let Some(cancel) = self.phase.cancel() {
                return Some(cancel);
            }
        }
        let quality = hand.map_or(Quality::Lost, |h| h.quality);
        let score = hand.and_then(|hand| all_score(hand, terms));
        self.phase.update(
            score,
            quality,
            Thresholds {
                enter: 1.0,
                exit: 0.0,
            },
        )
    }
}

#[derive(Default)]
pub struct Hold {
    all: All,
    started: Option<u64>,
}

impl Hold {
    pub fn update(
        &mut self,
        hand: Option<&HandState>,
        terms: &[Primitive],
        now_us: u64,
        duration: u64,
    ) -> bool {
        if !hand.is_some_and(|hand| hand.quality == Quality::Tracked) {
            self.started = None;
            self.all = All::default();
            return false;
        }
        match self.all.update(hand, terms) {
            Some(Phase::Began) => self.started = Some(now_us),
            Some(Phase::Ended | Phase::Cancelled) => self.started = None,
            _ => {}
        }
        if self
            .started
            .is_some_and(|start| now_us.saturating_sub(start) >= duration)
        {
            self.started = None;
            return true;
        }
        false
    }
}

#[derive(Clone, Copy)]
pub struct Tap {
    start_us: u64,
    origin: [f32; 2],
    unit: f32,
    moved_hu: f32,
}

impl Tap {
    pub fn new(start_us: u64, origin: [f32; 2], unit: f32) -> Self {
        Self {
            start_us,
            origin,
            unit: unit.max(1e-6),
            moved_hu: 0.0,
        }
    }

    pub fn observe(&mut self, point: [f32; 2]) {
        let moved = (point[0] - self.origin[0]).hypot(point[1] - self.origin[1]) / self.unit;
        self.moved_hu = self.moved_hu.max(moved);
    }

    pub fn moved(self) -> bool {
        self.moved_hu >= TAP_MOVE_HU
    }

    pub fn held(self, now_us: u64) -> bool {
        now_us.saturating_sub(self.start_us) > TAP_US
    }

    pub fn end(self, now_us: u64) -> Option<Phase> {
        (!self.held(now_us) && !self.moved()).then_some(Phase::Fired)
    }
}
