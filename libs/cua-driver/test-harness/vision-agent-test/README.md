# Vision-agent coordinate-invariant test

Tests cua-driver the way a **vision agent** actually hits it — and without the
overfit the modality recorder has (hand-tuned window-local points run through a
private ratio, which never exercises the driver's image→screen mapping).

## The invariant under test
**The pixel an agent reads off the returned screenshot is the pixel that gets
clicked** — verified by the target's own instrumented state changing.

## The loop (no cheating in locate/click)
1. **capture** — `get_window_state(capture_mode=vision)` (window) / `get_desktop_state`
   (desktop, true pixels): the exact image an agent receives.
2. **locate** — a deterministic pixel **in the returned-image coordinate space**
   (`PixelRegistryLocator`: a pre-measured pixel read off the real PNG, with a
   dims-guard that fails loud if the pinned geometry drifts). No AX `element_index`,
   no hand-converted window-local points. Pluggable `locate(image, target, dims)->(x,y)`.
3. **act** — `click`/`right_click`/`scroll` at that pixel (scope set to match capture).
4. **verify** — the harness oracle (`last_action=`, `clicks=`, …) — reliable pass/fail.
   A coordinate mis-map leaves the oracle unchanged → FAIL.

Run: `python3 vision_agent_test.py {wkwebview-click-window|wkwebview-click-desktop|appkit-click-window|safari-learnmore-desktop|all}`

## Two separate axes (don't conflate)
- **Driver coordinate invariant** (this test): deterministic locate → objective
  pass/fail → the regression guard. This is what turns the 2× Retina escape into
  a permanently-guarded one-liner.
- **Agent locator quality** (future, LLM): same `locate()` signature, send the PNG
  to a model, score localization hit-rate separately — never gates the coordinate
  regression.
- **Agent self-judged success with no oracle** (future, separate track): re-capture,
  ask the model "did it work", score its self-judgment vs the oracle. Kept isolated
  so a bad self-judge can't mask a driver regression.

## What the deterministic version caught (that the modality suite couldn't)
- **2× Retina desktop path now correct + guarded** — a read pixel (340,1358) in the
  3024×1964 desktop PNG converts to screen-point (170,679) and lands (oracle + a real
  Safari navigation confirm it).
- **Vision pixel-click is a no-op on AppKit `NSButton`** even frontmost — lands
  pixel-perfect (crosshair) but `NSButton`'s modal mouseDown loop reads the
  window-server queue, not the per-pid `CGEvent.postToPid` queue. The suite drives
  this via AXPress, so it never saw a vision agent's pixel click do nothing here.
- **Pixel path requires the target app frontmost** (the AX path doesn't).
- **AppKit harness window AX returns only the menu bar** — its `clicks=` oracle is
  unreachable that way; WKWebView exposes it fine.

## Full harness (scope)
Cross-product, each a one-line registry entry:
`{appkit, swiftui, wkwebview, electron, real-app} × {window, desktop(, secondary-display)} × {left, right, double, scroll, drag, type}`.
A color-fiducial locator (harness renders a unique-color dot per control) would make
the registry robust to window moves without OCR.
