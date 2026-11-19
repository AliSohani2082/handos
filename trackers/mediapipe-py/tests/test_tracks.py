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
