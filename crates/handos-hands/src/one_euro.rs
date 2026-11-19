use std::f32::consts::TAU;

#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub min_cutoff: f32,
    pub beta: f32,
    pub d_cutoff: f32,
}

pub const IMAGE: Params = Params {
    min_cutoff: 1.5,
    beta: 8.0,
    d_cutoff: 1.0,
};
pub const WORLD: Params = Params {
    beta: 15.0,
    ..IMAGE
};

pub struct OneEuro<const N: usize> {
    params: Params,
    t_us: Option<u64>,
    values: [f32; N],
    velocity: [f32; N],
}

impl<const N: usize> OneEuro<N> {
    pub fn new(params: Params) -> Self {
        Self {
            params,
            t_us: None,
            values: [0.0; N],
            velocity: [0.0; N],
        }
    }

    pub fn update(&mut self, sample: [f32; N], t_us: u64) -> [f32; N] {
        let Some(previous) = self.t_us.replace(t_us) else {
            self.values = sample;
            return sample;
        };
        let dt = (t_us.saturating_sub(previous) as f32 * 1e-6).max(0.001);
        let alpha = |cutoff| {
            let r = TAU * cutoff * dt;
            r / (1.0 + r)
        };
        for (i, raw) in sample.into_iter().enumerate() {
            let delta = raw - self.values[i];
            let dx = delta / dt;
            self.velocity[i] += alpha(self.params.d_cutoff) * (dx - self.velocity[i]);
            let weight = alpha(self.params.min_cutoff + self.params.beta * self.velocity[i].abs());
            self.values[i] += weight * delta;
        }
        self.values
    }

    pub fn velocity(&self) -> &[f32; N] {
        &self.velocity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settles_and_reduces_jitter() {
        let mut f = OneEuro::new(IMAGE);
        let mut energy = 0.0;
        f.update([0.0], 0);
        for i in 1..=100 {
            let value = if i % 2 == 0 { 0.002_f32 } else { -0.002 };
            energy += f.update([value], i * 66_667)[0].powi(2);
        }
        assert!((energy / 100.0).sqrt() < 0.001);
        let mut value = 0.0;
        for i in 101..=110 {
            value = f.update([0.5], i * 66_667)[0];
        }
        assert!((value - 0.5).abs() < 0.01);
    }
}
