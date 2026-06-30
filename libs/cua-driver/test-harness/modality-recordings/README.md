# cua-driver — modality recordings (Windows WPF + Linux GTK3)

Per-toolkit, per-modality recordings of cua-driver's no-foreground contract.
Each video: the **test harness on the LEFT**, the **`cua-driver-panel` dashboard on the RIGHT**
(always-on-top) showing the run's modality, a live **foreground/background indicator**, and a
**per-action measurement** of the contract (`held` vs `STOLE FOCUS`). The pink/cyan overlay is
cua-driver's per-session agent cursor (no real pointer move).

Five single-modality runs per toolkit:
- **ax-fg** — accessibility-tree actions, app intentionally kept FOREGROUND
- **ax-bg** — accessibility-tree actions, app should stay BACKGROUND (contract measured)
- **px-fg** — pixel-only (screenshot + coordinates), kept FOREGROUND
- **px-bg** — pixel-only, should stay BACKGROUND (contract measured)
- **px-desktop** — whole-screen, window-less screen-pixel actions (no window targeted)

Action set per run: left-click · double-click · right-click · drag · scroll ·
set_value (AX-only) · type · press-key.

Each action carries **two** independent measurements on the dashboard:
- **✓ worked / ✗ no-op** — did the action actually change the app's state (checkbox toggled,
  slider moved, text changed)? Verified by reading the harness's own state after each action.
- **held / STOLE FOCUS** — did the action keep the app in the background (the no-foreground contract)?

## Windows — WPF (`wpf-*.mp4`)
Recorded on the Windows VM (Session 2, 1024×768) via cua-driver `start_recording` (ffmpeg gdigrab).

| File | Measured |
|------|----------|
| `wpf-ax-fg.mp4` | foreground-mode, 8 actions |
| `wpf-ax-bg.mp4` | **1/8 actions stole focus** (double-click) |
| `wpf-px-fg.mp4` | foreground-mode, 7 actions |
| `wpf-px-bg.mp4` | **2/7 actions stole focus** (left-click, double-click) |
| `wpf-px-desktop.mp4` | foreground-mode, 4 actions |

## Linux — GTK3 (`gtk3-*.mp4`)
Recorded on the Ubuntu 24.04 VM under a headless **Xvfb + openbox + picom + AT-SPI** stack
(X11; picom composites the agent-cursor overlay), via cua-driver `start_recording` (x11grab).

| File | Effects landed | Focus contract |
|------|----------------|----------------|
| `gtk3-ax-fg.mp4` | 3/7 actions changed the app | foreground-mode, 8 actions |
| `gtk3-ax-bg.mp4` | 3/7 actions changed the app | **3/8 stole focus** |
| `gtk3-px-fg.mp4` | 1/6 actions changed the app | foreground-mode, 7 actions |
| `gtk3-px-bg.mp4` | 1/6 actions changed the app | **0/7 stole focus** |
| `gtk3-px-desktop.mp4` | 1/3 actions changed the app | foreground-mode, 4 actions |

**Key Linux finding (visible in the ✓/✗ column):** cua-driver injects Linux input via **XSendEvent**
(synthetic events delivered to a window without stealing focus — required for the no-foreground
contract). **GTK ignores synthetic XSendEvent events** (the `send_event` flag, a security feature),
so cua-driver's *pixel-based* actions (double-click, right-click, drag, scroll, and all vision-mode
clicks) **do not land** on the GTK harness — they're honestly marked ✗ no-op. The *AT-SPI / element*
actions (single-click, set_value, type) use accessibility actions, which GTK honors, so they ✓ work.
This is a real, documented driver behavior (see `platform-linux/src/input/mod.rs`), surfaced here by
the per-action verifier rather than hidden. On Windows/WPF the equivalent synthetic input *is* honored,
which is why those runs land all actions (e.g. the slider visibly moves).

## Reading the result
In the background runs, a `STOLE FOCUS` row means that action brought the app to the foreground
(contract violated for that action); `held` means it stayed in the background. The dashboard
tally at the bottom is the live count. Results differ by toolkit and modality — that variance
is the point: the recordings *measure* the contract honestly rather than asserting it holds.

### Notes / known limitations
- **Linux AT-SPI** exposes only buttons/text/checkbox roles (no sliders/scrollers) and no element
  frames, so drag/scroll and all vision-mode pixel actions are driven from harness-exported widget
  geometry. The drag/scroll actions fire and are measured but may not visibly move those widgets.
- **px-desktop** uses screen-absolute (window-less) clicks; under Xvfb these are not always
  delivered faithfully, so some clicks register as no-ops while still being measured.
- macOS (AppKit/SwiftUI) was out of scope for this pass (no recordable session on the host).
