//! Landmark replay for the overlay: the spike 1 JSONL trace, or a synthetic pair of hands.
//! Positions are sampled at display time by interpolating between tracking frames.

use std::time::Duration;

use serde::Deserialize;

/// Hands at one instant: 21 image-normalised [x, y] points each.
pub type Hands = Vec<[[f32; 2]; 21]>;

#[derive(Deserialize)]
struct Rec {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    t_ms: f64,
    #[serde(default)]
    hands: Vec<HandRec>,
}

#[derive(Deserialize)]
struct HandRec {
    lm: Vec<[f32; 3]>,
}

pub enum Replay {
    Trace { t_ms: Vec<f64>, frames: Vec<Hands> },
    Synthetic,
}

impl Replay {
    pub fn from_file(path: &str) -> std::io::Result<Replay> {
        let text = std::fs::read_to_string(path)?;
        let (mut t_ms, mut frames) = (Vec::new(), Vec::new());
        for line in text.lines() {
            let rec: Rec = serde_json::from_str(line).map_err(std::io::Error::other)?;
            if rec.kind != "frame" {
                continue;
            }
            let hands = rec
                .hands
                .iter()
                .filter(|h| h.lm.len() == 21)
                .map(|h| std::array::from_fn(|k| [h.lm[k][0], h.lm[k][1]]))
                .collect();
            t_ms.push(rec.t_ms);
            frames.push(hands);
        }
        if t_ms.len() < 2 {
            return Err(std::io::Error::other("trace has fewer than two frames"));
        }
        Ok(Replay::Trace { t_ms, frames })
    }

    pub fn synthetic() -> Replay {
        Replay::Synthetic
    }

    /// Hands at time `t` since start; the trace loops.
    pub fn sample(&self, t: Duration, out: &mut Hands) {
        out.clear();
        match self {
            Replay::Synthetic => {
                let s = t.as_secs_f32();
                for (k, phase) in [0.0f32, 2.1].into_iter().enumerate() {
                    let cx = 0.35 + 0.3 * k as f32 + 0.12 * (s * 0.9 + phase).sin();
                    let cy = 0.5 + 0.15 * (s * 1.3 + phase).cos();
                    out.push(open_hand(cx, cy, 0.09, (s * 3.0 + phase).sin() * 0.5 + 0.5));
                }
            }
            Replay::Trace { t_ms, frames } => {
                let span = t_ms[t_ms.len() - 1] - t_ms[0];
                let now = t_ms[0] + (t.as_secs_f64() * 1000.0) % span;
                let i = t_ms
                    .partition_point(|&x| x <= now)
                    .saturating_sub(1)
                    .min(t_ms.len() - 2);
                let f = ((now - t_ms[i]) / (t_ms[i + 1] - t_ms[i])).clamp(0.0, 1.0) as f32;
                let (a, b) = (&frames[i], &frames[i + 1]);
                for (k, ha) in a.iter().enumerate() {
                    match b.get(k) {
                        Some(hb) if a.len() == b.len() => out.push(std::array::from_fn(|j| {
                            [
                                ha[j][0] + (hb[j][0] - ha[j][0]) * f,
                                ha[j][1] + (hb[j][1] - ha[j][1]) * f,
                            ]
                        })),
                        _ => out.push(*ha),
                    }
                }
            }
        }
    }
}

/// An upright open hand around (cx, cy); `curl` in 0..1 bends the fingers.
fn open_hand(cx: f32, cy: f32, size: f32, curl: f32) -> [[f32; 2]; 21] {
    let mut p = [[0.0f32; 2]; 21];
    p[0] = [cx, cy + size];
    let bases = [
        (-0.45, 0.55),
        (-0.3, 0.0),
        (0.0, -0.05),
        (0.25, 0.0),
        (0.45, 0.15),
    ];
    for (finger, &(bx, by)) in bases.iter().enumerate() {
        let len = if finger == 0 { 0.35 } else { 0.4 } * size * (1.0 - 0.5 * curl);
        for seg in 0..4 {
            let j = 1 + finger * 4 + seg;
            let up = seg as f32 * len;
            p[j] = [cx + bx * size + (bx * 0.3) * up, cy + by * size - up];
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_interpolates_between_frames_and_loops() {
        let r = Replay::Trace {
            t_ms: vec![0.0, 100.0],
            frames: vec![vec![[[0.0, 0.0]; 21]], vec![[[1.0, 0.5]; 21]]],
        };
        let mut h = Vec::new();
        r.sample(Duration::from_millis(50), &mut h);
        assert_eq!(h[0][7], [0.5, 0.25]);
        r.sample(Duration::from_millis(150), &mut h);
        assert_eq!(h[0][7], [0.5, 0.25], "loops after the last frame");
    }

    #[test]
    fn hand_count_change_uses_the_earlier_frame() {
        let r = Replay::Trace {
            t_ms: vec![0.0, 100.0],
            frames: vec![
                vec![[[0.2, 0.2]; 21]],
                vec![[[0.9, 0.9]; 21], [[0.1, 0.1]; 21]],
            ],
        };
        let mut h = Vec::new();
        r.sample(Duration::from_millis(50), &mut h);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0][0], [0.2, 0.2]);
    }
}
