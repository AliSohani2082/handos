#![allow(clippy::expect_used)]

use handos_proto::{FLAG_WORLD, Hand, HandFrame, Handedness, decode_frame, encode_frame};

#[test]
fn golden_two_hands() {
    let points = std::array::from_fn(|i| std::array::from_fn(|axis| (i * 3 + axis) as f32 / 100.0));
    let frame = HandFrame {
        flags: FLAG_WORLD,
        seq: 7,
        t_capture_us: 1_000_000,
        t_emit_us: 1_027_000,
        hands: [Handedness::Left, Handedness::Right]
            .into_iter()
            .enumerate()
            .map(|(i, handedness)| Hand {
                track_id: 10 + i as u32,
                handedness,
                handedness_score: 0.875,
                presence: 1.0,
                landmarks: points,
                world: Some(points.map(|p| p.map(|v| -v))),
            })
            .collect(),
    };
    let mut packet = Vec::new();
    encode_frame(&frame, &mut packet).expect("encode golden frame");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/two_hands.bin");
    if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
        std::fs::write(path, &packet).expect("update golden packet");
    }
    let golden = std::fs::read(path).expect("read golden packet");
    assert_eq!(packet, golden);
    assert_eq!(decode_frame(&golden).expect("decode golden frame"), frame);
}
