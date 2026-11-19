use crate::{HandState, Landmarks, Quality};

const MAX_US: u64 = 33_000;

pub fn predict(hand: &HandState, target_us: u64) -> Landmarks {
    if hand.quality != Quality::Tracked {
        return hand.image;
    }
    let dt = target_us.saturating_sub(hand.t_capture_us).min(MAX_US) as f32 * 1e-6;
    std::array::from_fn(|i| {
        let [x, y, z] = hand.image[i];
        let [vx, vy] = hand.velocity[i];
        [x + vx * dt, y + vy * dt, z]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Role;

    #[test]
    fn prediction_clamps_and_skips_coasting() {
        let mut hand = HandState {
            role: Role::Right,
            track_id: 1,
            quality: Quality::Tracked,
            t_capture_us: 100_000,
            image: [[0.0; 3]; 21],
            world: None,
            velocity: [[1.0, -1.0]; 21],
        };
        assert_eq!(predict(&hand, 90_000), hand.image);
        let p = predict(&hand, 500_000)[0];
        assert!((p[0] - 0.033).abs() < 1e-6);
        hand.quality = Quality::Coasting;
        assert_eq!(predict(&hand, 500_000), hand.image);
    }
}
