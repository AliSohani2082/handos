/// One shape to draw, in the output's logical pixels with the origin at the top left.
/// Colours are straight (not premultiplied) RGBA in 0..=1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Prim {
    Line {
        a: [f32; 2],
        b: [f32; 2],
        width: f32,
        rgba: [f32; 4],
    },
    Dot {
        at: [f32; 2],
        radius: f32,
        rgba: [f32; 4],
    },
}

/// Everything drawn on one output for one frame. An empty list clears the output.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayList(pub Vec<Prim>);

impl DisplayList {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<Prim> for DisplayList {
    fn from_iter<I: IntoIterator<Item = Prim>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl Extend<Prim> for DisplayList {
    fn extend<I: IntoIterator<Item = Prim>>(&mut self, iter: I) {
        self.0.extend(iter);
    }
}
