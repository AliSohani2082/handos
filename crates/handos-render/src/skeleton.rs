use crate::Prim;

/// MediaPipe's 21-landmark hand topology.
pub const CONNECTIONS: [(usize, usize); 21] = [
    (0, 1),
    (1, 2),
    (2, 3),
    (3, 4),
    (0, 5),
    (5, 6),
    (6, 7),
    (7, 8),
    (5, 9),
    (9, 10),
    (10, 11),
    (11, 12),
    (9, 13),
    (13, 14),
    (14, 15),
    (15, 16),
    (13, 17),
    (17, 18),
    (18, 19),
    (19, 20),
    (0, 17),
];

/// How a skeleton is drawn, in logical pixels and straight RGBA.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub line_width: f32,
    pub joint_radius: f32,
    pub line_rgba: [f32; 4],
    pub joint_rgba: [f32; 4],
}

impl Default for Style {
    /// The spike's look: white bones, cyan joints.
    fn default() -> Self {
        Self {
            line_width: 3.0,
            joint_radius: 4.0,
            line_rgba: [1.0, 1.0, 1.0, 220.0 / 255.0],
            joint_rgba: [0.0, 200.0 / 255.0, 1.0, 1.0],
        }
    }
}

/// One hand's bones, then its joints on top, at `points` (logical pixels of the output).
pub fn skeleton(points: &[[f32; 2]; 21], style: Style) -> impl Iterator<Item = Prim> + '_ {
    let bones = CONNECTIONS.iter().map(move |&(a, b)| Prim::Line {
        a: points[a],
        b: points[b],
        width: style.line_width,
        rgba: style.line_rgba,
    });
    let joints = points.iter().map(move |&at| Prim::Dot {
        at,
        radius: style.joint_radius,
        rgba: style.joint_rgba,
    });
    bones.chain(joints)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skeleton_is_every_bone_then_every_joint() {
        let points = std::array::from_fn(|i| [i as f32, 2.0 * i as f32]);
        let prims: Vec<_> = skeleton(&points, Style::default()).collect();
        assert_eq!(prims.len(), CONNECTIONS.len() + 21);
        assert!(prims[..21].iter().all(|p| matches!(p, Prim::Line { .. })));
        assert!(prims[21..].iter().all(|p| matches!(p, Prim::Dot { .. })));
        assert_eq!(
            prims[4],
            Prim::Line {
                a: [0.0, 0.0],
                b: [5.0, 10.0],
                width: 3.0,
                rgba: Style::default().line_rgba
            }
        );
    }
}
