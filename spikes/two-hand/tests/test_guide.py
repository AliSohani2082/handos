"""The ghost guide must itself satisfy the detector it is teaching you to trigger."""

import numpy as np
import pytest

import guide
from analyze import Params, palm_touch, pluck
from handtrace import Hand


def as_hands(sc):
    out = []
    for label, px in zip(("Left", "Right"), sc["hands"]):
        lm = [[x / guide.W, y / guide.H, 0.0] for x, y in px]
        out.append(Hand(label, 1.0, lm, [[0.0, 0.0, 0.0]] * 21))
    return out


@pytest.mark.parametrize("mover", ["right", "left"])
def test_palm_guide_triggers_only_during_the_touch(mover):
    p = Params()
    ready = as_hands(guide.scene("palm", 0.5, mover))
    touch = as_hands(guide.scene("palm", 2.2, mover))
    away = as_hands(guide.scene("palm", 2.99, mover))
    assert not palm_touch(ready, p) and palm_touch(touch, p) and not palm_touch(away, p)


@pytest.mark.parametrize("mover", ["right", "left"])
def test_pluck_guide_triggers_only_while_pinching_the_fingertip(mover):
    p = Params()
    assert not pluck(as_hands(guide.scene("pluck", 0.5, mover)), p)
    assert not pluck(as_hands(guide.scene("pluck", 1.5, mover)), p)     # approaching, pinch still open
    assert pluck(as_hands(guide.scene("pluck", 2.2, mover)), p)
    assert not pluck(as_hands(guide.scene("pluck", 2.99, mover)), p)


def test_ready_phase_keeps_hands_apart_and_in_frame():
    for task in ("palm", "pluck"):
        a, b = guide.scene(task, 0.5)["hands"]
        assert np.linalg.norm(a[0] - b[0]) > 200
        for px in (a, b):
            assert px.shape == (21, 2) and px.min() > 0 and px[:, 0].max() < guide.W and px[:, 1].max() < guide.H


def test_draw_marks_the_frame_and_skips_done():
    img = np.full((guide.H, guide.W, 3), 40, np.uint8)
    guide.draw(img, "palm", "done", 0.0)
    assert (img == 40).all()
    guide.draw(img, "palm", "go", 2.2)
    assert (img != 40).any()
