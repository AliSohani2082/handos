import time

import numpy as np

import bench
from bench import BenchProbe, Phase, fmt_delta, parse_smaps, report, thread_group


def test_thread_group_names():
    assert thread_group(10, "python3\n", pid=10, main_comm="python3") == "main thread"
    assert thread_group(11, "python3\n", pid=10, main_comm="python3") == "unnamed threads"
    assert thread_group(12, "XNNPACK-3\n", pid=10, main_comm="python3") == "XNNPACK"
    assert thread_group(13, "QXcbEventQueue", pid=10, main_comm="python3") == "QXcbEventQueue"


def test_parse_smaps_groups_by_owner():
    lines = [
        "55d0-55e0 r-xp 00000000 08:01 1 /usr/bin/python3.13\n",
        "Rss:                2048 kB\n",
        "55e0-55f0 rw-p 00000000 00:00 0 [heap]\n",
        "Rss:                4096 kB\n",
        "7f00-7f10 rw-p 00000000 00:00 0 \n",
        "Rss:               10240 kB\n",
        "7f10-7f20 r-xp 00000000 08:01 2 /x/.venv/lib/python3.13/site-packages/mediapipe/tasks/c/libmp.so\n",
        "Rss:                3072 kB\n",
        "7f20-7f30 r-xp 00000000 08:01 3 /x/.venv/lib/python3.13/site-packages/opencv_python.libs/libQt.so\n",
        "Rss:                1024 kB\n",
        "7f30-7f40 r-xp 00000000 08:01 4 /usr/lib/libc.so.6\n",
        "Rss:                 512 kB\n",
    ]
    mem = parse_smaps(lines)
    assert next(iter(mem)).startswith("anonymous")
    assert mem["[heap] (Python objects, main malloc arena)"] == 4.0
    assert mem["mediapipe (wheel files)"] == 3.0
    assert mem["cv2 (wheel files)"] == 1.0
    assert mem["python runtime"] == 2.0
    assert mem["system libs"] == 0.5


def test_fmt_delta():
    assert fmt_delta(10, None) == "10"
    assert fmt_delta(10, 10) == "10 (=)"
    assert fmt_delta(8, 10) == "8 (-2, -20%)"


def _run(probe: BenchProbe, hands_for, max_s: float = 3.0) -> None:
    view = np.zeros((200, 300, 3), np.uint8)
    t_end = time.monotonic() + max_s
    while time.monotonic() < t_end:
        for s in bench.STAGES:
            with probe.stage(s):
                pass
        if not probe.frame(view, hands_for(probe), (640, 480)):
            return
        time.sleep(0.004)


def test_probe_measures_only_matching_windows(monkeypatch):
    monkeypatch.setattr(bench, "WINDOW_S", 0.05)
    phases = [Phase("warmup", 0.05, None, ""), Phase("idle", 0.2, 0, ""),
              Phase("ready", 0.05, None, ""), Phase("hands", 0.2, 2, "")]
    probe = BenchProbe(phases)
    _run(probe, lambda p: 2 if p.phase.name in ("ready", "hands") else 0)
    assert probe.done
    idle, hands = probe.acc["idle"].summary(), probe.acc["hands"].summary()
    assert idle["mean_hands"] == 0 and hands["mean_hands"] == 2
    assert idle["seconds"] >= 0.2 and hands["frames"] > 0
    assert set(idle["stages"]) == set(bench.STAGES)
    assert idle["memory_mib"] and idle["rss_mib"]["mean"] > 0
    assert "main thread" in idle["threads_cpu_pct"]


def test_probe_pauses_when_hands_do_not_match(monkeypatch):
    monkeypatch.setattr(bench, "WINDOW_S", 0.05)
    probe = BenchProbe([Phase("idle", 0.2, 0, "")])
    _run(probe, lambda p: 1, max_s=0.4)  # a hand is always in view: nothing may count
    assert not probe.done
    assert probe.acc["idle"].frames == 0


def test_report_renders_and_compares(monkeypatch):
    monkeypatch.setattr(bench, "WINDOW_S", 0.05)
    probe = BenchProbe([Phase("idle", 0.15, 0, "")])
    _run(probe, lambda p: 0)
    res = {"meta": {"label": "t", "git": "abc", "dirty": False, "time": "now"},
           "phases": {"idle": probe.acc["idle"].summary()}}
    text = report(res)
    assert "| CPU, % of one core |" in text and "| inference |" in text and "**RSS by owner**" in text
    assert "(=)" in report(res, res)
