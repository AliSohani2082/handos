#![allow(clippy::expect_used)]

use std::{fs::File, io::BufReader, path::Path};

use handos_gesture::{
    clutch::{Clutch, Mode},
    mapping::Mapping,
    phase::Phase,
    pointer::Pointer,
};
use handos_hands::{Conditioner, Hands, Role, features::Finger};
use handos_proto::{MSG_FRAME, decode_frame, trace::TraceReader};

#[test]
fn pluck_moving_hand_does_not_wake_during_repetitions() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../traces");
    let input = File::open(dir.join("pluck-good.htrace")).expect("open trace");
    let mut reader = TraceReader::new(BufReader::new(input)).expect("read header");
    let windows: serde_json::Value = serde_json::from_reader(
        File::open(dir.join("pluck-good.reps.json")).expect("open rep windows"),
    )
    .expect("parse rep windows");
    let intervals: Vec<_> = windows["reps"]
        .as_array()
        .expect("reps")
        .iter()
        .map(|w| w["start_us"].as_u64().expect("start")..=w["end_us"].as_u64().expect("end"))
        .collect();
    let hold = &windows["hold"][0];
    let hold =
        hold["start_us"].as_u64().expect("hold start")..=hold["end_us"].as_u64().expect("hold end");
    let mapping = Mapping::new([0.0; 2], [1.0; 2], 0.6, 0, [0.0; 2], [1920.0, 1080.0]);
    let mut results = [vec![false; intervals.len()], vec![false; intervals.len()]];
    let mut pointer = [Pointer::new(Role::Left, 1.0), Pointer::default()];
    let mut clutch = Clutch::default();
    let mut receiver_clutch = Clutch::default();
    let mut conditioner = Conditioner::default();
    let mut packet = vec![];
    let mut wakes = 0;
    let mut receiver_wakes = 0;
    let mut hold_ended = false;
    while reader.read_packet(&mut packet).expect("read packet") {
        if packet[0] != MSG_FRAME {
            continue;
        }
        let frame = decode_frame(&packet).expect("decode frame");
        let t = frame.t_capture_us;
        let hands = conditioner.update(&frame, t);
        // The other hand in this M0 recording is deliberately held open during the exercise.
        let moving = Hands {
            right: hands.right.clone(),
            ..Hands::default()
        };
        if !hold_ended && t > *hold.end() {
            hold_ended = true;
            clutch.set_mode(Mode::Sleep, t);
        }
        if clutch
            .update(&moving, t)
            .is_some_and(|m| m.0 == Mode::Default)
        {
            wakes += usize::from(hold_ended);
        }
        receiver_wakes += usize::from(
            receiver_clutch
                .update(hands, t)
                .is_some_and(|m| m.0 == Mode::Default),
        );
        for (i, role) in [Role::Left, Role::Right].into_iter().enumerate() {
            let out = pointer[i].update(hands.get(role), mapping, t, true);
            if out.events.contains(&(Finger::Index, Phase::Began))
                && let Some(index) = intervals.iter().position(|interval| interval.contains(&t))
            {
                results[i][index] = true;
            }
        }
    }
    let counts = results.map(|hits| hits.iter().filter(|&&hit| hit).count());
    eprintln!(
        "3D pinch begins (left, right): {counts:?}; moving-hand wakes during repetitions: {wakes}"
    );
    for pointer in &mut pointer {
        pointer.cancel();
        assert!(pointer.pressed().is_empty());
    }
    assert_eq!(wakes, 0);
    assert!(
        receiver_wakes > 0,
        "the exercise includes a genuine open-palm wake"
    );
}
