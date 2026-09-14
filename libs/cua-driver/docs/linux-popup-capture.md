# X11 exact-window screenshots with owned combo popups

## Reproduction

On an X11 desktop, open a disposable spreadsheet with a cell-validation dropdown
containing three choices. Bring its document window to the foreground and open
the dropdown. Capture that exact window through the driver while the list is
visible, then compare with the screen. Some toolkits create the popup as an
override-redirect root sibling, not part of the target drawable: the exact-window
image omits the list even though it is visible on screen.

The original case used LibreOffice Calc and Pass/Fail/Held choices. Two driver
window captures surrounding an independent desktop observation were identical
and black in the popup region, while the desktop showed the choices. The
[original public SDK reproduction](https://github.com/tanishqkancharla/opensky/blob/33b4e9cf6d6329215206d5a78571198743216d3a/e2e/specs/linux-dropdown-popup.test.ts)
is historical supporting evidence, not certification of this upstream port.

## Existing and proposed behavior

Existing capture reads only the requested X11 drawable. The proposed foreground
path composes a matching COMBO popup into that same image, clipped to its original
canvas. It does not enlarge the image or read desktop pixels. Background-window
capture and Wayland keep the raw path.

A popup qualifies only when it is viewable, opaque, unshaped, borderless, 24-bit,
override-redirect, has exactly the target's transient owner and PID, and is above
the target frame in the root stack. Foreign occlusion, unsupported shape/opacity,
ambiguous metadata and changes between pre/post inventories refuse the capture.
The inventory is bounded to 256 root children and 32 target ancestors. These
conditions deliberately leave unsupported popup forms as explicit limitations.
Pixel freshness is not atomic: unchanged geometry/ownership cannot prove that
an application's pixels did not change between reads.

## Reproducible fixture checks

The included live Rust tests create actual X11 windows with known background and
popup colors, invoke production capture, decode the image, and assert its pixels.
They cover exact ownership and wrong siblings, missing/hidden popups, background
capture, multiple popup stack order, clipping, and shape/opacity/occlusion/resize
refusals. They must run only in a disposable depth-24 Xvfb desktop with a real
EWMH window manager, never on a user's desktop.

From the repository root, in an already prepared disposable Linux environment:

```sh
cd libs/cua-driver/rust
cargo test --locked -p platform-linux --lib capture::popup::tests
CUA_POPUP_CAPTURE_DISPOSABLE_X11=1 cargo test --locked -p platform-linux --lib \
  capture::popup::live_tests -- --ignored --test-threads=1
```

The canonical Linux desktop gate runs from the repository root:

```sh
scripts/ci/linux/run-rust-e2e.sh
```

Local macOS host checks exclude this Linux capture module. The exact candidate
must pass Linux compile, seven pure tests, four live Xvfb tests, the app-level
reproduction and canonical desktop gates before readiness. Pixel composition
does not establish popup click delivery; that is a separate input workstream.
