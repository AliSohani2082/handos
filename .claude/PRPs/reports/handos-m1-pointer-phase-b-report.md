# M1 Pointer Phase B

The hand crate filters landmarks, assigns stable left and right roles, and holds the last pose for up to 150 ms when tracking drops. It keeps the world points in camera coordinates and mirrors the image points once. A frame without world points can still move the pointer. It cannot produce a pinch or a wake gesture.

The gesture crate produces commands for movement, left and right click, drag, and vertical scroll. Index and middle pinches share the pointer claim. Sleep or hand loss cancels the claim and releases a dragged button. If an open palm is visible when the system goes to sleep, it must leave the wake pose before it can wake the system again.

| Tasks | Result |
|---|---|
| B1–B5 | Filter, outlier, coast, and prediction tests passed. Both traces kept stable roles. Median palm-facing scores passed the 0.2 limit for both hands. |
| B6–B8 | Claims, phase transitions, mapping, sleep, wake, and the wake delay passed. The moving hand produced zero wakes during the recorded pluck repetitions. |
| B9 | Click, drag, scroll, and button release passed at 15 and 30 fps with controlled samples. The M0 pluck recording did not meet the 3D pinch target. |

## Replay limits

The pluck recording asks the user to hold the receiving palm open. This is a valid wake pose under M1 rules. The replay checks the moving hand during the exercise and checks that the full recording can wake. The setup period also contains an open palm before the annotated hold.

M0 scored the pinch in the camera image. In 49 of 50 repetitions, the moving hand reached an image gap of 0.30 screen hand units or less. In 3D, the smallest gap across all repetitions was 0.437 world hand units. M1's default pinch enters at 0.30 world hand units, so this trace produced zero qualifying pinches. Keep the default until a new recording of actual thumb-to-index clicks can establish whether it fits the user's hand. Task D7 adds calibration.

The workspace passed formatting, Clippy, all 27 Rust tests, and the release build. Phase C will connect these commands to the Wayland pointer and overlay. Desktop behavior and the live click target remain unverified.
