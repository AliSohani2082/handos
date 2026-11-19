import numpy as np

from stabilizer import HandObs, HandStabilizer, OneEuro


def obs(label, x, score=0.95):
    lm = np.zeros((21, 3))
    lm[:, 0] = x
    lm[:, 1] = 0.5
    return HandObs(label, score, lm, lm.copy())


def test_label_stays_through_short_flicker():
    s = HandStabilizer()
    for k in range(60):
        raw = "Left" if k % 3 == 0 and k > 5 else "Right"  # model flips every 3rd frame
        out = s.update([obs(raw, 0.5)], k / 30)
        assert out[0].label == "Right"
        assert out[0].id == 1


def test_label_flips_after_sustained_disagreement():
    s = HandStabilizer()
    s.update([obs("Right", 0.5)], 0)
    labels = [s.update([obs("Left", 0.5)], k / 30)[0].label for k in range(1, 40)]
    assert labels[0] == "Right" and labels[-1] == "Left"


def test_two_hands_keep_their_tracks_and_never_share_a_label():
    s = HandStabilizer()
    for k in range(30):
        out = s.update([obs("Left", 0.2 + k * 0.005), obs("Left", 0.8 - k * 0.005)], k / 30)
        assert sorted(h.label for h in out) == ["Left", "Right"]
        assert sorted(h.id for h in out) == [1, 2]


def test_track_survives_brief_dropout():
    s = HandStabilizer()
    s.update([obs("Right", 0.5)], 0)
    for k in range(1, 5):
        assert s.update([], k / 30) == []
    assert s.update([obs("Left", 0.52)], 5 / 30)[0].label == "Right"


def test_a_far_jump_never_creates_a_duplicate_label():
    s = HandStabilizer()
    s.update([obs("Left", 0.2), obs("Right", 0.8)], 0)
    out = s.update([obs("Left", 0.2), obs("Right", 0.5)], 1 / 30)  # Right jumps 0.3 > max_jump
    assert sorted(h.label for h in out) == ["Left", "Right"]


def test_a_dropout_resets_the_disagreement_run():
    s = HandStabilizer()
    s.update([obs("Right", 0.5)], 0)
    k = 1
    for _ in range(3):  # three runs of 20 disagreeing frames, each broken by a dropout
        for _ in range(20):
            assert s.update([obs("Left", 0.5)], k / 30)[0].label == "Right"
            k += 1
        s.update([], k / 30)
        k += 1


def test_two_swapped_labels_correct_together():
    s = HandStabilizer()
    s.update([obs("Left", 0.2), obs("Right", 0.8)], 0)
    for k in range(1, 40):
        out = s.update([obs("Right", 0.2), obs("Left", 0.8)], k / 30)
        assert sorted(h.label for h in out) == ["Left", "Right"]
    assert {round(float(h.lm[0, 0]), 1): h.label for h in out} == {0.2: "Right", 0.8: "Left"}


def test_one_euro_reduces_jitter_on_still_input():
    rng = np.random.default_rng(0)
    f = OneEuro(1.5, 8.0)
    raw = 0.5 + rng.normal(0, 0.005, 200)
    out = np.array([f(np.array([v]), k / 30)[0] for k, v in enumerate(raw)])
    assert out[50:].std() < raw[50:].std() / 2
