#![allow(clippy::expect_used)]

use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    process::Command,
};

use handos_proto::{MSG_FRAME, decode_frame, trace::TraceReader};

fn trace(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../traces")
        .join(name)
}

#[test]
fn both_spike_traces_preserve_frames_and_holds() {
    // The palm recording starts the hold prompt before both hands enter the frame.
    for (name, expected_frames, expected_hold) in [
        ("palm-good", 2412, (22, 60)),
        ("pluck-good", 2406, (60, 60)),
    ] {
        let path = trace(&format!("{name}.htrace"));
        let input = File::open(&path).expect("open fixture");
        let mut reader = TraceReader::new(BufReader::new(input)).expect("read fixture header");
        let windows: serde_json::Value = serde_json::from_reader(
            File::open(path.with_extension("reps.json")).expect("open windows"),
        )
        .expect("read windows");
        let holds = windows["hold"].as_array().expect("hold windows");
        assert!(!holds.is_empty());
        let mut packet = Vec::new();
        let (mut frames, mut hold, mut two_hands) = (0, 0, 0);
        while reader
            .read_packet(&mut packet)
            .expect("read fixture packet")
        {
            assert_eq!(packet[0], MSG_FRAME);
            let frame = decode_frame(&packet).expect("decode fixture frame");
            assert_eq!(frame.seq, frames);
            frames += 1;
            if holds.iter().any(|window| {
                let start = window["start_us"].as_u64().expect("hold start");
                let end = window["end_us"].as_u64().expect("hold end");
                (start..=end).contains(&frame.t_capture_us)
            }) {
                hold += 1;
                two_hands += u32::from(frame.hands.len() == 2);
            }
        }
        assert_eq!(frames, expected_frames, "{name}: frame count");
        assert_eq!((two_hands, hold), expected_hold, "{name}: hold coverage");
    }
}

#[test]
fn headless_cli_replays_without_tracker_or_display() {
    let out = Command::new(env!("CARGO_BIN_EXE_handos"))
        .args(["trace", "replay"])
        .arg(trace("pluck-good.htrace"))
        .args(["--headless", "--speed", "0"])
        .env("HANDOS_LOG", "info")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR")
        .output()
        .expect("spawn handos");
    let log = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{log}");
    assert!(
        log.contains("replay complete") && log.contains("frames"),
        "{log}"
    );
}
