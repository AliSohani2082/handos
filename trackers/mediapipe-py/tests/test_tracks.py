from handos_tracker_mp.tracks import Associate


def points(x, y):
    return [[x, y, 0]] * 21


def test_tracks_survive_reordering_and_brief_dropout():
    associate = Associate(max_missed=1)
    left, right = associate.update([points(0.1, 0.1), points(0.8, 0.1)])
    assert associate.update([points(0.8, 0.2), points(0.1, 0.2)]) == [right, left]
    assert associate.update([points(0.1, 0.2)]) == [left]
    assert associate.update([points(0.8, 0.2)]) == [right]
    assert associate.update([]) == []
    assert associate.update([points(0.1, 0.2), points(0.2, 0.6)])[0] != left


def test_both_hands_keep_their_ids_when_moving_the_same_way():
    associate = Associate()
    a, b = associate.update([points(0.30, 0.5), points(0.50, 0.5)])
    # Nearest-first would give hand 0 track b (0.05 away) and leave hand 1 without a track.
    assert associate.update([points(0.45, 0.5), points(0.65, 0.5)]) == [a, b]
