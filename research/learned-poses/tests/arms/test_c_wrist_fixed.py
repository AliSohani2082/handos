from types import SimpleNamespace

import numpy as np
import pytest

from learned_poses.arms.c_wrist_fixed import NONE, WristFixedArm, wrist_frame


def hand(curl: float = 0.0) -> np.ndarray:
    """A right hand in metres: palm in the x-y plane, fingers along +y. `curl` bends the
    fingertips towards the palm (along -z)."""
    bases = [(0.03, 0.03), (0.025, 0.09), (0.0, 0.095), (-0.02, 0.09), (-0.04, 0.08)]
    p = np.zeros((21, 3))
    for f, (bx, by) in enumerate(bases):
        for j in range(4):
            k = 1 + 4 * f + j
            p[k] = [bx + (0.012 * j if f == 0 else 0.0), by + 0.025 * j, -curl * j * j * 0.004]
    return p


def random_rotation(rng: np.random.Generator) -> np.ndarray:
    q, r = np.linalg.qr(rng.normal(size=(3, 3)))
    q = q * np.sign(np.diag(r))
    if np.linalg.det(q) < 0:  # a proper rotation, not a reflection
        q[:, 0] = -q[:, 0]
    return q


def sample(world: np.ndarray | None, handedness: str = "right") -> SimpleNamespace:
    return SimpleNamespace(
        image=np.zeros((21, 3)), world=world, handedness=handedness, t_us=0, label=""
    )


def test_the_frame_is_wrist_origin_middle_knuckle_up_in_hand_units():
    f = wrist_frame(hand(), "right")
    np.testing.assert_allclose(f[0], 0.0, atol=1e-12)
    assert abs(f[9][0]) < 1e-9 and abs(f[9][2]) < 1e-9 and f[9][1] > 0
    assert np.linalg.norm(f[5] - f[17]) == pytest.approx(1.0)


def test_features_are_the_same_under_any_3d_rotation_shift_and_scale():
    arm = WristFixedArm()
    rng = np.random.default_rng(0)
    for curl in (0.0, 1.0):
        still = arm.featurize(sample(hand(curl)))
        for _ in range(200):
            r = random_rotation(rng)
            moved = hand(curl) @ r.T * rng.uniform(0.5, 2.0) + rng.normal(size=3)
            np.testing.assert_allclose(arm.featurize(sample(moved)), still, atol=1e-5)


def test_a_left_hand_is_mirrored_into_the_same_shape():
    right = hand(0.5)
    left = right * np.array([-1.0, 1.0, 1.0])
    np.testing.assert_allclose(wrist_frame(left, "left"), wrist_frame(right, "right"), atol=1e-12)


def test_missing_world_landmarks_and_degenerate_hands_are_refused():
    arm = WristFixedArm()
    with pytest.raises(ValueError, match="world"):
        arm.featurize(sample(None))
    with pytest.raises(ValueError, match="degenerate"):
        arm.featurize(sample(np.zeros((21, 3))))


def test_probabilities_list_poses_in_label_order_then_none():
    rng = np.random.default_rng(1)
    arm = WristFixedArm()
    shapes = {"rock": 0.0, "fist": 1.5, NONE: 0.75}
    X, y = [], []
    for label, curl in shapes.items():
        for _ in range(30):
            moved = hand(curl) @ random_rotation(rng).T + rng.normal(scale=0.002, size=(21, 3))
            X.append(arm.featurize(sample(moved)))
            y.append(label)
    arm.fit(np.array(X), np.array(y), seed=7)
    assert arm.classes_ == ["rock", "fist", NONE]
    proba = arm.predict_proba(np.array(X))
    assert proba.shape == (90, 3)
    np.testing.assert_allclose(proba.sum(axis=1), 1.0, atol=1e-6)
    assert (proba.argmax(axis=1) == np.repeat([0, 1, 2], 30)).mean() > 0.9


def test_none_has_a_column_even_without_none_samples():
    arm = WristFixedArm()
    X = np.array([arm.featurize(sample(hand(c))) for c in (0.0, 0.1, 1.4, 1.5)])
    arm.fit(X, np.array(["open", "open", "fist", "fist"]), seed=0)
    assert arm.classes_ == ["open", "fist", NONE]
    assert np.all(arm.predict_proba(X)[:, 2] == 0.0)


def test_the_same_seed_trains_the_same_model():
    arm_a, arm_b = WristFixedArm(), WristFixedArm()
    X = np.array([arm_a.featurize(sample(hand(c))) for c in (0.0, 0.1, 1.4, 1.5)])
    y = np.array(["open", "open", "fist", "fist"])
    arm_a.fit(X, y, seed=3)
    arm_b.fit(X, y, seed=3)
    np.testing.assert_array_equal(arm_a.predict_proba(X), arm_b.predict_proba(X))
