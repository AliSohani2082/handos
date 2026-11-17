#!/usr/bin/env bash
# Runs a command and prints how much CPU Hyprland used over the same time, as % of one core.
# A GPU overlay is fully damaged every frame, so part of its cost may land in the compositor.
#   spikes/overlay-gpu/measure.sh <command...>
set -u
pid=$(pidof Hyprland || pidof hyprland) || { echo "Hyprland not running" >&2; exit 1; }
ticks() { awk '{print $14 + $15}' "/proc/$pid/stat"; }
hz=$(getconf CLK_TCK)
t0=$(date +%s.%N); c0=$(ticks)
"$@"; rc=$?
t1=$(date +%s.%N); c1=$(ticks)
awk -v c="$((c1 - c0))" -v hz="$hz" -v t="$t0" -v u="$t1" \
    'BEGIN { printf "   Hyprland CPU over the whole run: %.2f%% of one core\n", c / hz / (u - t) * 100 }'
exit $rc
