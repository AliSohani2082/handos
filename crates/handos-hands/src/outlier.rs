use crate::{Landmarks, features::hu_screen};

#[derive(Default)]
pub struct Outlier {
    last: Option<Landmarks>,
    rejected: u8,
}

impl Outlier {
    pub fn accept(&mut self, points: &Landmarks) -> bool {
        if self.rejected < 2
            && self.last.as_ref().is_some_and(|last| {
                let limit = hu_screen(last).max(1e-6);
                points
                    .iter()
                    .zip(last)
                    .any(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]) > limit)
            })
        {
            self.rejected += 1;
            return false;
        }
        self.rejected = 0;
        self.last = Some(*points);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_third_consecutive_jump() {
        let mut gate = Outlier::default();
        let mut original = [[0.0; 3]; 21];
        original[5][0] = 0.1;
        let shifted = original.map(|[x, y, z]| [x + 0.2, y, z]);
        assert!(gate.accept(&original));
        assert!(!gate.accept(&shifted));
        assert!(!gate.accept(&shifted));
        assert!(gate.accept(&shifted));
        assert!(gate.accept(&shifted));
    }
}
