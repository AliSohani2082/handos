use crate::{HandState, Landmarks, Role};

const PALM: [usize; 5] = [0, 5, 9, 13, 17];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finger {
    Thumb,
    Index,
    Middle,
    Ring,
    Pinky,
}

impl Finger {
    const ALL: [Self; 5] = [
        Self::Thumb,
        Self::Index,
        Self::Middle,
        Self::Ring,
        Self::Pinky,
    ];

    fn tip(self) -> usize {
        4 + self as usize * 4
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
    Open,
    Fist,
    Other,
}

#[derive(Clone, Copy)]
pub struct Pinch {
    pub strength: f32,
    pub point: [f32; 2],
}

pub fn palm_center(points: &Landmarks) -> [f32; 2] {
    std::array::from_fn(|axis| {
        PALM.iter().map(|&i| points[i][axis]).sum::<f32>() / PALM.len() as f32
    })
}

pub fn hu_screen(points: &Landmarks) -> f32 {
    let distance =
        |a: usize, b: usize| (points[a][0] - points[b][0]).hypot(points[a][1] - points[b][1]);
    distance(5, 17).max(0.8 * distance(0, 9))
}

fn difference(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

fn length([x, y, z]: [f32; 3]) -> f32 {
    x.hypot(y).hypot(z)
}

pub fn hu_world(points: &Landmarks) -> f32 {
    length(difference(points[5], points[17]))
}

pub fn facing(hand: &HandState) -> Option<f32> {
    let world = hand.world.as_ref()?;
    let [ax, ay, az] = difference(world[5], world[0]);
    let [bx, by, bz] = difference(world[17], world[0]);
    let normal = [ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx];
    let norm = length(normal);
    let sign = if hand.role == Role::Left { -1.0 } else { 1.0 };
    (norm > 1e-9).then(|| (1.0 + sign * normal[2] / norm) * 0.5)
}

pub fn pinch_strength(distance_hu: f32) -> f32 {
    ((0.60 - distance_hu) / 0.40).clamp(0.0, 1.0)
}

pub fn pinch(hand: &HandState, finger: Finger) -> Option<Pinch> {
    let world = hand.world.as_ref()?;
    let unit = hu_world(world);
    if unit <= 1e-6 || finger == Finger::Thumb {
        return None;
    }
    let tip = finger.tip();
    Some(Pinch {
        strength: pinch_strength(length(difference(world[4], world[tip])) / unit),
        point: std::array::from_fn(|axis| (hand.image[4][axis] + hand.image[tip][axis]) * 0.5),
    })
}

pub fn extension(points: &Landmarks, finger: Finger) -> Option<f32> {
    let joint = finger.tip() - 2;
    let a = difference(points[joint - 1], points[joint]);
    let b = difference(points[joint + 1], points[joint]);
    let scale = length(a) * length(b);
    if scale <= 1e-9 {
        return None;
    }
    let dot = a.iter().zip(b).map(|(a, b)| a * b).sum::<f32>();
    let angle = (dot / scale).clamp(-1.0, 1.0).acos().to_degrees();
    Some(((angle - 90.0) / 70.0).clamp(0.0, 1.0))
}

pub fn pose_score(hand: &HandState, pose: Pose) -> Option<f32> {
    let world = hand.world.as_ref()?;
    match pose {
        Pose::Open => Finger::ALL.iter().try_fold(1.0_f32, |score, &finger| {
            Some(score.min(extension(world, finger)?))
        }),
        Pose::Fist => Finger::ALL[1..].iter().try_fold(1.0_f32, |score, &finger| {
            Some(score.min(1.0 - extension(world, finger)?))
        }),
        Pose::Other => Some(1.0 - pose_score(hand, Pose::Open)?.max(pose_score(hand, Pose::Fist)?)),
    }
}

pub fn pose(hand: &HandState) -> Pose {
    for pose in [Pose::Open, Pose::Fist] {
        if pose_score(hand, pose).is_some_and(|score| score >= 0.7) {
            return pose;
        }
    }
    Pose::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinch_strength_endpoints() {
        assert!((pinch_strength(0.2) - 1.0).abs() < 1e-6);
        assert_eq!(pinch_strength(0.6), 0.0);
    }
}
