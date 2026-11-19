#![allow(clippy::expect_used)]

use std::{collections::HashMap, fs::File, io::BufReader, path::Path};

use handos_hands::{Conditioner, Quality, Role, features::facing};
use handos_proto::{MSG_FRAME, decode_frame, trace::TraceReader};

#[test]
fn trace_roles_and_palm_facing() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../traces");
    for name in ["palm-good", "pluck-good"] {
        let input = File::open(dir.join(format!("{name}.htrace"))).expect("open trace");
        let mut reader = TraceReader::new(BufReader::new(input)).expect("read header");
        let windows: serde_json::Value = serde_json::from_reader(
            File::open(dir.join(format!("{name}.reps.json"))).expect("open hold windows"),
        )
        .expect("parse hold windows");
        let hold = &windows["hold"][0];
        let start = hold["start_us"].as_u64().expect("start");
        let end = hold["end_us"].as_u64().expect("end");
        let mut packet = vec![];
        let mut conditioner = Conditioner::default();
        let mut observed = HashMap::new();
        let mut scores = [Vec::new(), Vec::new()];
        let mut both = 0;
        while reader.read_packet(&mut packet).expect("read packet") {
            if packet[0] != MSG_FRAME {
                continue;
            }
            let frame = decode_frame(&packet).expect("decode frame");
            let t = frame.t_capture_us;
            let hands = conditioner.update(&frame, t);
            if (start..=end).contains(&t)
                && [&hands.left, &hands.right]
                    .iter()
                    .all(|h| h.as_ref().is_some_and(|h| h.quality == Quality::Tracked))
            {
                both += 1;
            }
            for (index, role) in [Role::Left, Role::Right].into_iter().enumerate() {
                let Some(hand) = hands.get(role) else {
                    continue;
                };
                assert!(
                    observed
                        .insert(hand.track_id, role)
                        .is_none_or(|last| last == role),
                    "{name}: track {} changed roles",
                    hand.track_id
                );
                if name == "palm-good"
                    && (start..=end).contains(&t)
                    && hand.quality == Quality::Tracked
                    && let Some(score) = facing(hand)
                {
                    scores[index].push(score);
                }
            }
        }
        assert!(both >= 15, "{name}: both roles in {both} hold frames");
        if name == "palm-good" {
            for scores in &mut scores {
                scores.sort_by(f32::total_cmp);
                assert!(
                    !scores.is_empty() && scores[scores.len() / 2] < 0.2,
                    "{scores:?}"
                );
            }
        }
    }
}
