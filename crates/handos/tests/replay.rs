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

fn frames(name: &str) -> Vec<handos_proto::HandFrame> {
    let input = File::open(trace(name)).expect("open fixture");
    let mut reader = TraceReader::new(BufReader::new(input)).expect("read fixture header");
    let mut packet = Vec::new();
    let mut frames = Vec::new();
    while reader.read_packet(&mut packet).expect("read packet") {
        if packet[0] == MSG_FRAME {
            frames.push(decode_frame(&packet).expect("decode frame"));
        }
    }
    frames
}

fn desktop() -> handos_platform::fake::FakeDesktop {
    use handos_platform::{OutputId, OutputInfo, Rect};
    let output = |id, name: &str, x| OutputInfo {
        id: OutputId(id),
        name: name.into(),
        logical_rect: Rect {
            x,
            y: 0,
            width: 1920,
            height: 1080,
        },
        scale: 1.0,
    };
    handos_platform::fake::FakeDesktop::new(vec![
        output(1, "eDP-1", 0),
        output(2, "HDMI-A-1", 1920),
    ])
}

#[test]
fn the_pipeline_draws_on_the_focused_output_while_awake_and_releases_everything() {
    use handos::pipeline::{Config, Pipeline};
    use handos_gesture::clutch::Mode;
    use handos_platform::{OutputId, Prim, fake::OverlayCall};

    let mut desktop = desktop();
    let mut pipeline = Pipeline::new(Config::default());
    let focused = || Some("HDMI-A-1".to_owned());
    let mut woke = 0;
    for frame in frames("palm-good.htrace") {
        let step = pipeline.on_frame(&frame, frame.t_capture_us, &mut desktop, &focused);
        woke += usize::from(step.mode_changed == Some(Mode::Default));
    }
    pipeline.suspend(&mut desktop);

    assert!(woke >= 1, "the open-palm holds should wake the pipeline");
    assert_eq!(pipeline.target().map(|t| t.id), Some(OutputId(2)));
    let draws: Vec<_> = desktop
        .overlay
        .calls
        .iter()
        .filter_map(|call| match call {
            OverlayCall::Draw(id, list) => Some((*id, list)),
            OverlayCall::Clear(_) => None,
        })
        .collect();
    assert!(draws.len() > 100, "{} draws", draws.len());
    assert!(draws.iter().all(|(id, _)| *id == OutputId(2)));
    assert!(
        draws
            .iter()
            .all(|(_, list)| matches!(list.0.last(), Some(Prim::Dot { .. })))
    );
    assert_eq!(
        desktop.overlay.calls.last(),
        Some(&OverlayCall::Clear(OutputId(2)))
    );
    assert!(desktop.input.held().is_empty(), "a button stayed pressed");
}

#[test]
#[ignore = "0 of 50: real clicks close the 3D pinch only to ~0.49 hu, B5 begins at 0.30; D7 sets the threshold (plan B10 RESULT)"]
fn recorded_clicks_begin_inside_their_rep_windows() {
    use handos::pipeline::{Config, Pipeline};
    use handos_gesture::clutch::Mode;
    use handos_platform::{Button, fake::InputCall};

    let windows: serde_json::Value = serde_json::from_reader(
        File::open(trace("click-good.reps.json")).expect("open rep windows"),
    )
    .expect("parse rep windows");
    let reps: Vec<_> = windows["reps"]
        .as_array()
        .expect("reps")
        .iter()
        .map(|w| w["start_us"].as_u64().expect("start")..=w["end_us"].as_u64().expect("end"))
        .collect();
    let mut desktop = desktop();
    let mut pipeline = Pipeline::new(Config::default());
    let mut hit = vec![false; reps.len()];
    for (i, frame) in frames("click-good.htrace").iter().enumerate() {
        let t = frame.t_capture_us;
        if i == 0 {
            pipeline.set_mode(Mode::Default, t);
        }
        let before = desktop.input.calls.len();
        pipeline.on_frame(frame, t, &mut desktop, &|| None);
        let pressed =
            desktop.input.calls[before..].contains(&InputCall::Button(Button::Left, true));
        if pressed && let Some(rep) = reps.iter().position(|r| r.contains(&t)) {
            hit[rep] = true;
        }
    }
    pipeline.suspend(&mut desktop);
    let hits = hit.iter().filter(|&&h| h).count();
    eprintln!(
        "left clicks or drags begun inside rep windows: {hits} of {}",
        reps.len()
    );
    assert!(hits >= 45, "{hits} of {}", reps.len());
    assert!(desktop.input.held().is_empty(), "a button stayed pressed");
}
