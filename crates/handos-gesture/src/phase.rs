use handos_hands::Quality;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Began,
    Changed,
    Ended,
    Cancelled,
    Fired,
}

#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    pub enter: f32,
    pub exit: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PhaseMachine {
    primed: bool,
    active: bool,
}

impl PhaseMachine {
    pub fn update(
        &mut self,
        score: Option<f32>,
        quality: Quality,
        limits: Thresholds,
    ) -> Option<Phase> {
        if quality == Quality::Lost || (quality == Quality::Tracked && score.is_none()) {
            return self.cancel();
        }
        if quality == Quality::Coasting {
            self.primed = false;
            return None;
        }
        let Some(score) = score.filter(|x| x.is_finite()) else {
            return self.cancel();
        };
        if self.active {
            if score <= limits.exit {
                self.active = false;
                return Some(Phase::Ended);
            }
            return Some(Phase::Changed);
        }
        if score < limits.enter {
            self.primed = false;
        } else if self.primed {
            self.primed = false;
            self.active = true;
            return Some(Phase::Began);
        } else {
            self.primed = true;
        }
        None
    }

    pub fn cancel(&mut self) -> Option<Phase> {
        let active = self.active;
        *self = Self::default();
        active.then_some(Phase::Cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const LIMITS: Thresholds = Thresholds {
        enter: 0.75,
        exit: 0.45,
    };

    #[test]
    fn hysteresis_and_loss() {
        let mut state = PhaseMachine::default();
        assert_eq!(state.update(Some(0.8), Quality::Tracked, LIMITS), None);
        assert_eq!(
            state.update(Some(0.8), Quality::Tracked, LIMITS),
            Some(Phase::Began)
        );
        assert_eq!(
            state.update(Some(0.6), Quality::Tracked, LIMITS),
            Some(Phase::Changed)
        );
        assert_eq!(state.update(Some(0.2), Quality::Coasting, LIMITS), None);
        assert_eq!(
            state.update(Some(0.45), Quality::Tracked, LIMITS),
            Some(Phase::Ended)
        );
        assert_eq!(state.update(None, Quality::Lost, LIMITS), None);
    }

    proptest! {
        #[test]
        fn phase_pairs(samples in proptest::collection::vec((0.0_f32..=1.0, 0_u8..3), 0..256)) {
            let mut phase = PhaseMachine::default();
            let mut active = false;
            for (score, q) in samples {
                let q = [Quality::Tracked, Quality::Coasting, Quality::Lost][usize::from(q)];
                match phase.update(Some(score), q, LIMITS) {
                    Some(Phase::Began) => {
                        prop_assert!(!active);
                        active = true;
                    }
                    Some(Phase::Changed) => prop_assert!(active),
                    Some(Phase::Ended | Phase::Cancelled) => {
                        prop_assert!(active);
                        active = false;
                    }
                    _ => {}
                }
            }
        }
    }
}
