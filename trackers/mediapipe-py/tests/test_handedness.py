from handos_tracker_mp.tracker import anatomical_handedness


def test_raw_camera_label_conversion():
    assert anatomical_handedness("Left") == 0
    assert anatomical_handedness("Right") == 1
