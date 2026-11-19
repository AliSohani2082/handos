/// Both the box and input points use mirrored image coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Mapping {
    min: [f32; 2],
    max: [f32; 2],
    hand_scale: f32,
    output: u32,
    origin: [f32; 2],
    size: [f32; 2],
}

impl Mapping {
    pub fn new(
        min: [f32; 2],
        max: [f32; 2],
        hand_scale: f32,
        output: u32,
        origin: [f32; 2],
        size: [f32; 2],
    ) -> Option<Self> {
        let valid = (0..2).all(|i| {
            let span = max[i] - min[i];
            span.is_finite()
                && span > 0.0
                && size[i] > 0.0
                && origin[i].is_finite()
                && (origin[i] + size[i]).is_finite()
        }) && hand_scale.is_finite()
            && hand_scale >= 0.0;
        valid.then_some(Self {
            min,
            max,
            hand_scale,
            output,
            origin,
            size,
        })
    }

    pub fn output(self) -> u32 {
        self.output
    }

    pub fn point(self, palm: [f32; 2], point: [f32; 2]) -> [f32; 2] {
        std::array::from_fn(|i| {
            let scale = self.size[i] / (self.max[i] - self.min[i]);
            let centre = ((palm[i] - self.min[i]) * scale).clamp(0.0, self.size[i]);
            self.origin[i]
                + (centre + self.hand_scale * (point[i] - palm[i]) * scale).clamp(0.0, self.size[i])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_corners_clamps_and_matches_plain_mapping_at_full_scale() {
        let map = Mapping::new([0.0; 2], [1.0; 2], 1.0, 2, [10.0, 20.0], [200.0, 100.0])
            .unwrap_or_else(|| unreachable!());
        assert_eq!(map.point([0.0; 2], [0.0; 2]), [10.0, 20.0]);
        assert_eq!(map.point([1.0; 2], [1.0; 2]), [210.0, 120.0]);
        assert_eq!(map.point([0.5; 2], [0.25, 0.75]), [60.0, 95.0]);
        assert_eq!(map.point([0.5; 2], [-1.0, 2.0]), [10.0, 120.0]);
    }
}
