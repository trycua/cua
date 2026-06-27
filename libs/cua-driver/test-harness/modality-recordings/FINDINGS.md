# Modality recordings — findings (cua-driver behavior surfaced by the per-action verifier)

These recordings run the 8-action matrix (click, double-click, right-click, drag, scroll,
set_value, type, press-key) against a controlled harness on Windows (WPF) and Linux (GTK3), in
5 modalities each (ax-fg, ax-bg, vision-fg, vision-bg, vision-desktop). Each action now carries
**two independent measurements**:

- **✓ worked / ✗ no-op** — did the action change the harness's own state (verified by reading the
  harness status: `agreed=`, `slider_value=`, `last_action=`, `mirror=`, `menu_action=`,
  `scroll_offset=`). Read via UIAutomation on Windows, via a harness state file on Linux.
- **held / STOLE FOCUS** — the no-foreground contract.

Principle followed: the harness/recorder stays honest (no overfitting); real gaps are documented
here as cua-driver work, to be fixed in the driver first and then re-tested.

## What the verifier surfaced (to triage as driver work)

### Windows / WPF (ax-bg, representative) — after fixes: 5/7 land
| action | effect | note |
|--------|--------|------|
| left-click checkbox | ✓ worked | UIA toggle |
| double-click button | ✓ worked | last_action=double_click |
| right-click | ✗ no-op (clean) | **Root cause: off-screen target, not a driver coordinate bug.** At 1024×768 the panel forces the harness to 556px wide, so the form reflows taller than the screen and the right-click button lands at y≈786 (below the screen). The synthetic tap previously clamped onto the **taskbar** (opened its menu). **DRIVER FIX:** `point_in_window_bounds()` now refuses a click whose resolved point is outside its window and returns a clear error — no more taskbar misfire (frame-verified). To actually *exercise* right-click needs a screen tall enough to show the control (this RDP session won't take a programmatic resize). |
| drag slider | ✓ worked | slider_value 0→48 |
| scroll | ✗ no-op (clean) | Same off-screen root cause (scroll-tall pane is lower still); now guarded, not misfiring. |
| set_value | ✓ worked | mirror=set-by-cua |
| type | ✓ worked | **Fixed:** the recorder now clicks the text box (focus) before `type_text`; type targets the focused control, and `set_value` doesn't focus, so without this it was a no-op. |
| press-key (Tab) | n/a | no asserted effect |

**Driver fix shipped (this branch):** `platform-windows` — `point_in_window_bounds()` + guards in the
click / double_click / right_click element paths. Investigated and ruled out: UIA `GetClickablePoint`
for cursor-centering (returns the same rect center or fails), and SetFocus-to-scroll-into-view (would
steal foreground, breaking the very contract these bg runs measure).

### Linux / GTK3
- AT-SPI/element actions (single-click, set_value, type) **✓ work**.
- All **pixel-based** actions (double-click, right-click, drag, scroll, every vision-mode click)
  **✗ no-op**: cua-driver's Linux input is **XSendEvent** (synthetic, no focus steal — required for
  the contract), but **GTK ignores synthetic XSendEvent** (`send_event` flag). Documented in
  `platform-linux/src/input/mod.rs`. Real limitation; not a recorder bug.

## Agent-cursor centering (user-reported)
The agent cursor lands off-center on the checkbox. Root cause: the WPF checkbox's AX
BoundingRectangle is the **full-width row** (≈803px), so its center is mid-row. Verified that
UIA `GetClickablePoint` does **not** help — it either fails (slider) or returns the same rect
center (text box). So this is **not a driver defect**: the bounds are reported faithfully and the
element action works regardless of cursor position. The visual is a function of the control's
geometry (a stretched CheckBox). Most-honest fix would be the harness laying the checkbox out at
content width (how real apps render it); a driver left-bias heuristic would be unreliable.

## Deliverables
10 verified recordings (5 WPF + 5 GTK3) + index, published at
`https://cuademo06261324.blob.core.windows.net/demo/modality-videos/index.html`.
