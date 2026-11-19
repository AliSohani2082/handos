#![allow(clippy::expect_used)]

use handos_gesture::{
    clutch::{Clutch, Mode},
    mapping::Mapping,
    pointer::{Button, Pointer, PointerCmd},
};
use handos_hands::{HandState, Hands, Quality, Role};
use proptest::prelude::*;

fn hand(index: bool, middle: bool, x: f32, y: f32) -> HandState {
    let mut world = [[0.0, 0.0, 0.0]; 21];
    world[5] = [0.05, -0.05, 0.0];
    world[17] = [-0.05, -0.05, 0.0];
    world[8] = [if index { 0.02 } else { 0.1 }, 0.0, 0.0];
    world[12] = [0.0, if middle { 0.02 } else { 0.1 }, 0.0];
    let mut image = [[x, y, 0.0]; 21];
    image[5][0] += 0.1;
    image[17][0] -= 0.1;
    HandState {
        role: Role::Right,
        track_id: 1,
        quality: Quality::Tracked,
        image,
        world: Some(world),
        velocity: [[0.0; 2]; 21],
        t_capture_us: 0,
    }
}

fn mapping() -> Option<Mapping> {
    Mapping::new([0.0; 2], [1.0; 2], 0.6, 1, [0.0; 2], [1920.0, 1080.0])
}

fn button(btn: Button, pressed: bool) -> PointerCmd {
    PointerCmd::Button { btn, pressed }
}

#[test]
fn click_drag_scroll_and_cancel_at_both_frame_rates() {
    for dt in [33_333, 66_667] {
        let mut pointer = Pointer::default();
        let closed = hand(true, false, 0.5, 0.5);
        let open = hand(false, false, 0.5, 0.5);
        pointer.update(Some(&closed), mapping(), 0, true);
        let begin = pointer.update(Some(&closed), mapping(), dt, true);
        assert!(!begin.commands.contains(&button(Button::Left, true)));
        let click = pointer.update(Some(&open), mapping(), dt * 2, true);
        assert!(
            click
                .commands
                .ends_with(&[button(Button::Left, true), button(Button::Left, false)])
        );

        pointer.update(Some(&closed), mapping(), dt * 3, true);
        pointer.update(Some(&closed), mapping(), dt * 4, true);
        let moved = hand(true, false, 0.6, 0.5);
        let drag = pointer.update(Some(&moved), mapping(), dt * 5, true);
        assert!(matches!(
            drag.commands.as_slice(),
            [
                PointerCmd::Move { .. },
                PointerCmd::Button {
                    btn: Button::Left,
                    pressed: true
                },
                PointerCmd::Move { .. }
            ]
        ));
        assert!(!pointer.pressed().is_empty());
        assert_eq!(
            pointer.update(None, mapping(), dt * 6, true).commands,
            [button(Button::Left, false)]
        );
        assert!(pointer.pressed().is_empty());

        let middle = hand(false, true, 0.5, 0.5);
        pointer.update(Some(&middle), mapping(), dt * 7, true);
        pointer.update(Some(&middle), mapping(), dt * 8, true);
        assert!(
            pointer
                .update(Some(&open), mapping(), dt * 9, true)
                .commands
                .ends_with(&[button(Button::Right, true), button(Button::Right, false)])
        );
        pointer.update(Some(&middle), mapping(), dt * 10, true);
        pointer.update(Some(&middle), mapping(), dt * 11, true);
        assert!(
            pointer
                .update(Some(&middle), mapping(), dt * 11 + 310_000, true)
                .commands
                .is_empty()
        );
        let lower = hand(false, true, 0.5, 0.55);
        let scroll = pointer.update(Some(&lower), mapping(), dt * 12 + 310_000, true);
        assert!(matches!(scroll.commands.as_slice(), [PointerCmd::Axis { dy }] if *dy > 0.0));
        assert_eq!(pointer.cancel().commands, [PointerCmd::AxisStop]);
    }
}

#[test]
fn a_drag_that_moves_on_its_last_frame_drops_at_the_new_position() {
    for dt in [33_333, 66_667] {
        let mut pointer = Pointer::default();
        let closed = hand(true, false, 0.5, 0.5);
        pointer.update(Some(&closed), mapping(), 0, true);
        pointer.update(Some(&closed), mapping(), dt, true);
        let held = pointer.update(Some(&closed), mapping(), dt + 310_000, true);
        assert!(held.commands.contains(&button(Button::Left, true)));

        let released = pointer.update(
            Some(&hand(false, false, 0.7, 0.5)),
            mapping(),
            dt * 2 + 310_000,
            true,
        );
        let [PointerCmd::Move { x, .. }, release] = released.commands.as_slice() else {
            panic!(
                "expected one move then the release, got {:?}",
                released.commands
            );
        };
        assert_eq!(*release, button(Button::Left, false));
        let last_held = held.commands.iter().rev().find_map(|c| match c {
            PointerCmd::Move { x, .. } => Some(*x),
            _ => None,
        });
        assert!(last_held.is_some_and(|held_x| *x > held_x));
        assert!(pointer.pressed().is_empty());
    }
}

fn pose_hand(open: bool) -> HandState {
    let mut hand = hand(false, false, 0.5, 0.5);
    let mut world = [[0.0; 3]; 21];
    for (f, start) in [1, 5, 9, 13, 17].into_iter().enumerate() {
        let x = 0.08 - f as f32 * 0.04;
        world[start] = [x, -0.05, 0.0];
        world[start + 1] = [x, -0.08, 0.0];
        world[start + 2] = if open {
            [x, -0.11, 0.0]
        } else {
            [x, -0.07, -0.03]
        };
        world[start + 3] = [x, -0.14, 0.0];
    }
    hand.world = Some(world);
    hand
}

#[test]
fn clutch_hold_and_grace_at_both_frame_rates() {
    for dt in [33_333, 66_667] {
        let mut clutch = Clutch::default();
        let open = Hands {
            right: Some(pose_hand(true)),
            ..Hands::default()
        };
        let fist = Hands {
            right: Some(pose_hand(false)),
            ..Hands::default()
        };
        let mut now = 0;
        while now < 500_000 {
            assert!(clutch.update(&open, now).is_none());
            now += dt;
        }
        while clutch.update(&open, now).is_none() {
            now += dt;
            assert!(now <= 700_000);
        }
        assert_eq!(clutch.mode(), Mode::Default);
        assert!(!clutch.pointer_enabled(now + 299_999));
        assert!(clutch.pointer_enabled(now + 300_000));
        let begins = now + dt;
        now = begins;
        while clutch.update(&fist, now).is_none() {
            now += dt;
            assert!(now - begins <= 1_200_000);
        }
        assert!(now - begins >= 1_000_000);
        assert_eq!(clutch.mode(), Mode::Sleep);
        assert!(clutch.toggle(now).is_some());
        let empty = Hands::default();
        assert!(clutch.update(&empty, now + 29_999_999).is_none());
        assert_eq!(
            clutch
                .update(&empty, now + 30_000_000)
                .expect("auto sleep")
                .0,
            Mode::Sleep
        );
    }
}

#[test]
fn an_open_palm_at_sleep_must_be_lowered_before_it_wakes_again() {
    let mut clutch = Clutch::default();
    let open = Hands {
        right: Some(pose_hand(true)),
        ..Hands::default()
    };
    clutch.set_mode(Mode::Default, 0);
    clutch.update(&open, 1);
    clutch.set_mode(Mode::Sleep, 2);
    for i in 1..20 {
        assert!(clutch.update(&open, i * 66_667).is_none());
    }
    clutch.update(&Hands::default(), 1_500_000);
    for i in 0..9 {
        assert!(clutch.update(&open, 1_600_000 + i * 66_667).is_none());
    }
    assert_eq!(
        clutch
            .update(&open, 2_200_003)
            .expect("wake after lowering")
            .0,
        Mode::Default
    );
}

proptest! {
    #[test]
    fn every_cancellation_releases_buttons(
        sequence in proptest::collection::vec((any::<bool>(), any::<bool>(), 0.3_f32..0.7), 0..100)
    ) {
        let mut pointer = Pointer::default();
        let mut held = false;
        for (frame, (index, middle, x)) in sequence.into_iter().enumerate() {
            for cmd in pointer.update(Some(&hand(index, middle, x, 0.5)), mapping(), frame as u64 * 66_667, true).commands {
                if let PointerCmd::Button { btn: Button::Left, pressed } = cmd {
                    prop_assert_ne!(held, pressed);
                    held = pressed;
                }
            }
        }
        let cancelled = pointer.update(None, mapping(), 9_000_000, false);
        prop_assert_eq!(cancelled.commands.contains(&button(Button::Left, false)), held);
        prop_assert!(pointer.pressed().is_empty());
    }
}
