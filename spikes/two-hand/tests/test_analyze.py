import math

from analyze import (OneEuro, Params, assign_tracks, evaluate_reps, longest_dropout_ms, palm_touch, pluck,
                     sustained)
from handtrace import Frame, Hand, Rep, Trace


def make_hand(label: str, cx: float, cy: float, size: float = 0.1, index_tip=None, thumb_tip=None) -> Hand:
    """A flat open hand: wrist at (cx, cy + size), fingers pointing up."""
    lm = [[cx, cy + size, 0.0] for _ in range(21)]
    lm[9] = [cx, cy, 0.0]                      # middle MCP: palm size == size
    for k, dx in zip((5, 13, 17), (-0.3, 0.3, 0.6)):
        lm[k] = [cx + dx * size, cy, 0.0]
    for k, dx in zip((4, 8, 12, 16, 20), (-0.8, -0.3, 0.0, 0.3, 0.6)):
        lm[k] = [cx + dx * size, cy - size, 0.0]
    if index_tip:
        lm[8] = [*index_tip, 0.0]
    if thumb_tip:
        lm[4] = [*thumb_tip, 0.0]
    wl = [[0.0, 0.0, 0.0] for _ in range(21)]
    return Hand(label, 0.99, lm, wl)


def test_one_euro_settles_on_constant_input_and_smooths_noise():
    f = OneEuro(min_cutoff=1.0, beta=0.0)
    out = [f(t / 30, [100.0 + (5 if t % 2 else -5), 50.0]) for t in range(300)]
    assert abs(out[-1][0] - 100.0) < 3.0
    assert math.isclose(out[-1][1], 50.0)


def test_one_euro_reset_restarts_from_input():
    f = OneEuro(1.0, 0.0)
    f(0.0, [0.0])
    f.reset()
    assert f(1.0, [42.0]) == [42.0]


def test_tracks_follow_position_not_order_and_flag_label_swap():
    left, right = make_hand("Left", 0.3, 0.5), make_hand("Right", 0.7, 0.5)
    frames = [
        Frame(0, [left, right]),
        Frame(33, [right, left]),                                          # order flips: no swap
        Frame(66, [make_hand("Right", 0.3, 0.5), make_hand("Left", 0.7, 0.5)]),  # labels flip: swap
    ]
    tracks, swaps = assign_tracks(frames)
    assert swaps == [False, False, True]
    assert tracks[0][0].label == "Left" and tracks[1][0].label == "Left"


def test_two_hands_with_same_label_count_as_swap():
    _, swaps = assign_tracks([Frame(0, [make_hand("Left", 0.3, 0.5), make_hand("Left", 0.7, 0.5)])])
    assert swaps == [True]


def test_longest_dropout_measures_until_recovery_or_window_end():
    two = [make_hand("Left", 0.3, 0.5), make_hand("Right", 0.7, 0.5)]
    one = two[:1]
    frames = [Frame(0, two), Frame(33, one), Frame(66, one), Frame(100, two), Frame(133, one)]
    assert longest_dropout_ms(frames, end_ms=150) == 67
    assert longest_dropout_ms(frames, end_ms=400) == 267


def test_palm_touch_needs_index_tip_on_the_other_palm():
    receiver = make_hand("Left", 0.5, 0.5)
    centre = (0.5 + (-0.3 + 0.3 + 0.6) * 0.1 / 5, 0.5 + 0.1 / 5)  # mean of the five palm points
    toucher = make_hand("Right", 0.8, 0.5, index_tip=centre)
    away = make_hand("Right", 0.8, 0.5)
    p = Params()
    assert palm_touch([receiver, toucher], p)
    assert not palm_touch([receiver, away], p)
    assert not palm_touch([receiver], p)


def test_pluck_needs_a_pinch_near_a_fingertip_of_the_other_hand():
    target = make_hand("Left", 0.3, 0.5)
    tip = target.lm[8][:2]
    plucker = make_hand("Right", 0.7, 0.5, index_tip=(tip[0] + 0.002, tip[1]), thumb_tip=(tip[0] - 0.002, tip[1]))
    open_hand = make_hand("Right", 0.7, 0.5)
    assert pluck([target, plucker], Params())
    assert not pluck([target, open_hand], Params())


def test_sustained_requires_consecutive_frames():
    assert sustained([False, True, True, False], 2)
    assert not sustained([True, False, True, False], 2)


def test_rep_passes_only_when_recognised_without_swap_or_long_dropout():
    receiver = make_hand("Left", 0.5, 0.5)
    centre = (0.5 + 0.012, 0.5 + 0.02)
    toucher = make_hand("Right", 0.8, 0.5, index_tip=centre)
    good = [Frame(t, [receiver, toucher]) for t in range(0, 2000, 33)]
    trace = Trace({"task": "palm"}, frames=good, reps=[Rep(0, "palm", 0, 2000)])
    [r] = evaluate_reps(trace, Params())
    assert r.passed and r.recognised and r.swaps == 0 and r.longest_dropout_ms == 0

    gappy = [f if not (500 <= f.t_ms < 800) else Frame(f.t_ms, [receiver]) for f in good]
    [r] = evaluate_reps(Trace({"task": "palm"}, frames=gappy, reps=[Rep(0, "palm", 0, 2000)]), Params())
    assert r.recognised and not r.passed and r.longest_dropout_ms > 150


def test_written_trace_round_trips_through_the_cli(tmp_path):
    import json

    import analyze
    from handtrace import TraceWriter

    receiver = make_hand("Left", 0.5, 0.5)
    toucher = make_hand("Right", 0.8, 0.5, index_tip=(0.512, 0.52))
    path = tmp_path / "palm-good.jsonl"
    with open(path, "w") as fp:
        w = TraceWriter(fp)
        w.header(task="palm", light="good")
        for t in range(0, 7000, 33):
            w.frame(Frame(t, [receiver, toucher] if t >= 4000 else [receiver, make_hand("Right", 0.8, 0.5)], 9.0))
        w.hold(0, 4000)
        w.rep(Rep(0, "palm", 4000, 6000))
    out = tmp_path / "summary.json"
    assert analyze.main([str(path), "--json", str(out)]) == 0
    [s] = json.loads(out.read_text())
    assert s["passed"] == 1 and s["verdict"] == "GO" and s["raw_jitter_px"] == 0.0
    assert s["filter_best"]["jitter_px"] <= 3.0


def test_rep_with_no_frames_fails():
    [r] = evaluate_reps(Trace({"task": "palm"}, frames=[], reps=[Rep(0, "palm", 0, 2000)]), Params())
    assert not r.passed and math.isclose(r.longest_dropout_ms, 2000)
