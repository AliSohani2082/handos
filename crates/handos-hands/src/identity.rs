use std::collections::{HashMap, HashSet};

use handos_proto::{Hand, Handedness};

use crate::{Landmarks, quality::MIN_PRESENCE};

pub type Role = Handedness;
const GRACE_US: u64 = 500_000;
const MIN_SCORE: f32 = 0.8;
const CONFIRM_FRAMES: u8 = 3;

pub const fn index(role: Role) -> usize {
    match role {
        Role::Left => 0,
        Role::Right => 1,
    }
}

#[derive(Default)]
struct Track {
    last_seen: u64,
    candidate: Option<(Role, u8)>,
}

#[derive(Default)]
pub struct Identity {
    tracks: HashMap<u32, Track>,
    owners: [Option<u32>; 2],
}

impl Identity {
    pub fn update(&mut self, hands: &[Hand], now_us: u64) -> [Option<u32>; 2] {
        self.tracks
            .retain(|_, track| now_us.saturating_sub(track.last_seen) < GRACE_US);
        for owner in &mut self.owners {
            if owner.is_some_and(|id| !self.tracks.contains_key(&id)) {
                *owner = None;
            }
        }

        let visible: Vec<_> = hands
            .iter()
            .filter(|hand| hand.presence >= MIN_PRESENCE)
            .collect();
        let overlap = visible.iter().enumerate().any(|(i, a)| {
            visible[i + 1..]
                .iter()
                .any(|b| a.track_id != b.track_id && overlaps(&a.landmarks, &b.landmarks))
        });
        let mut seen = HashSet::with_capacity(visible.len());
        for hand in visible {
            if !seen.insert(hand.track_id) {
                continue;
            }
            let track = self.tracks.entry(hand.track_id).or_default();
            track.last_seen = now_us;
            if self.owners.contains(&Some(hand.track_id)) {
                continue;
            }
            if overlap || hand.handedness_score < MIN_SCORE {
                track.candidate = None;
                continue;
            }
            let n = match track.candidate {
                Some((role, n)) if role == hand.handedness => (n + 1).min(CONFIRM_FRAMES),
                _ => 1,
            };
            track.candidate = Some((hand.handedness, n));
            if n == CONFIRM_FRAMES {
                let preferred = index(hand.handedness);
                let free = if self.owners[preferred].is_none() {
                    preferred
                } else {
                    1 - preferred
                };
                if self.owners[free].is_none() {
                    self.owners[free] = Some(hand.track_id);
                }
            }
        }
        for (id, track) in &mut self.tracks {
            if !seen.contains(id) {
                track.candidate = None;
            }
        }
        self.owners
    }
}

fn bounds(points: &Landmarks) -> ([f32; 2], [f32; 2]) {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for point in points {
        for axis in 0..2 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    (min, max)
}

fn overlaps(a: &Landmarks, b: &Landmarks) -> bool {
    let (amin, amax) = bounds(a);
    let (bmin, bmax) = bounds(b);
    (0..2).all(|axis| amin[axis] <= bmax[axis] && bmin[axis] <= amax[axis])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hand(id: u32, x: f32) -> Hand {
        Hand {
            track_id: id,
            handedness: Role::Right,
            handedness_score: 0.9,
            presence: 1.0,
            landmarks: [[x, 0.0, 0.0]; 21],
            world: None,
        }
    }

    #[test]
    fn overlap_and_flicker_leave_roles_stable() {
        let mut identity = Identity::default();
        for now in 0..3 {
            assert_eq!(
                identity.update(&[hand(1, 0.0), hand(2, 0.0)], now),
                [None; 2]
            );
        }
        let both = [hand(1, 0.0), hand(2, 1.0)];
        for now in 3..5 {
            assert_eq!(identity.update(&both, now), [None; 2]);
        }
        assert_eq!(identity.update(&both, 5), [Some(2), Some(1)]);
        let mut swapped = both;
        swapped[0].handedness = Role::Left;
        assert_eq!(identity.update(&swapped, 6), [Some(2), Some(1)]);
        assert_eq!(identity.update(&[], 500_005), [Some(2), Some(1)]);
        assert_eq!(identity.update(&[], 500_006), [None; 2]);
    }
}
