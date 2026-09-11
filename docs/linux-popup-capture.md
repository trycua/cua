# Linux dropdown capture

Status: reproduced observation gap; bounded X11 candidate implemented, compilation and real candidate acceptance pending.

This work is selected by the fork owner's Linux Computer Use parity goal. The fork
has issues disabled, so this document and the linked draft pull request retain the
problem, scope and evidence. The existing earlier PR stack is separate.

## User-visible problem

Opening Calc's Pass/Fail/Held validation dropdown produces a black rectangle in
OpenSky's exact-window screenshot. The actual desktop displays all three choices.
An agent therefore cannot read the menu from that screenshot, even though the
keyboard can select a valid choice.

The reproduction uses the public SDK and original OSWorld workbook, with driver
source `ed9fd15e39a1d2acd07c4f3078440c3ab2ad44f2`, certified binary SHA256
`8b50bfbdbd9c98aaa5d0ff9d4f2196b18115b96fe6c2d68847ddb16c21ce6692`.
Merged base `53b2a81deaa7119a31a0e68bbfc0962e1652bf90` has the same Git tree.

SDK diagnostic source:
[DROPDOWN-D01](https://github.com/tanishqkancharla/opensky/blob/33b4e9cf6d6329215206d5a78571198743216d3a/e2e/specs/linux-dropdown-popup.test.ts).
It brackets a read-only desktop capture with two public app observations, with no
intervening input. Both app PNGs are byte-identical and black in the popup region;
the desktop PNG visibly contains Pass, Fail and Held. Active PID/XID and owned
window geometry remain unchanged. The popup keeps its XID, location and width,
but its height settles from 71 to 61 pixels between metadata reads. These are
sequential observations, not an atomic or fully geometry-stable capture.

The disposable diagnostic image preserves all 402 original package versions and
base layers, adding ImageMagick and its seven required dependencies only. Image
ID: `sha256:2e3a8a80cf737938d0b550c7943f5a9604d088df917e536fe3ab552932c71331`.
Owned app/container cleanup and unchanged saved workbook were verified. Original
agent failures and scores remain unchanged.

## Implementation scope

Investigate composition of proven same-window popup surfaces into the existing
X11 screenshot canvas before resize. Preserve its coordinate origin and bounds.
Do not substitute a desktop crop or alter background-window capture behavior.

`list_windows` cannot be used as a popup inventory: the EWMH list omits unmanaged
windows, and existing discovery filters empty titles. A bounded root-child walk
can obtain geometry and properties, but same PID/class/client leader alone does
not associate a popup with a particular sibling document. The D02 metadata discriminator now proves the observed COMBO popup has the same
PID as the exact workbook, with WM_TRANSIENT_FOR pointing directly at that XID.
It is viewable and override-redirect; these properties remain stable while the
height settles from 71 to 61. This evidence selects the bounded implementation
below; same-PID/class discovery alone is still insufficient.

Only include a popup after proving its relationship to the requested window,
viewability, stacking and stable identity/geometry. Preserve conservative behavior
for missing ownership, unsupported alpha/shape and window replacement. Clip any
owned popup pixels to the original canvas. This work targets X11; it does not
establish macOS, Windows or Wayland popup parity.

## Acceptance

- Public SDK screenshot exposes the actual three choices in this Calc workflow.
- Independent desktop observation agrees with the visible popup and its placement.
- Wrong-process and same-process sibling popups remain excluded.
- Missing/conflicting ownership, closing/replaced popup, changing geometry and
  popup bounds outside the target canvas have explicit checked outcomes.
- Existing background capture and action-coordinate behavior remain intact.
- Run focused tests while implementing; then the canonical Linux suites on the
  exact final candidate, followed by merge-tree and installer/start/cleanup smoke.

No production fix, candidate acceptance or new agent campaign score is claimed.

## Candidate implementation and limits

`capture/popup.rs` wraps the existing raw drawable capture only on X11. It first
requires `_NET_ACTIVE_WINDOW` to equal the requested XID and a verifiable target
PID. A bounded inventory (at most256 root children,32 ancestor steps) accepts only
one viewable root-child COMBO whose PID matches and whose `WM_TRANSIENT_FOR` is
the exact target. Unmapped, unrelated, sibling-window, missing-owner and non-COMBO
windows are not composited. Known combos with ambiguous type, unsupported geometry
or multiple matching popups produce an explicit capture error.

The popup must be override-redirect, depth24, borderless, rectangular and opaque.
X Shape bounding/clip flags and `_NET_WM_WINDOW_OPACITY` are checked; captured
alpha must also be fully opaque. Shape-extension failure is an explicit limitation,
not permission to guess. Root stacking must place the popup above the target's
actual reparented frame. Any mapped drawable above it overlapping its rectangle
causes refusal. Metadata, target ancestry, root child order, foreground identity,
geometry and mapped surfaces above the popup are re-read after both direct-XID
captures. Known popup disappearance or any checked change returns an error rather
than silently delivering the original black region. No input, activation, sleep,
recursive popup capture or desktop pixels are used.

Composition uses root-coordinate differences and clips into the original window
canvas before existing resize logic. It never expands the screenshot/action frame.
Popup parts outside the window remain outside this contract. Background requests,
no eligible popup, and all existing Wayland/XWayland dispatch behavior retain the
raw capture path. Foreground no-popup inspection adds bounded metadata queries;
its cost has not been measured. Missing/ambiguous ownership stays excluded rather
than broadening the target. This means some real application popups remain
unsupported.

The readback is not an atomic X-server snapshot. A window destroyed and recreated
with identical XID/properties between all checks is not distinguishable by these
metadata primitives; this candidate does not claim a security boundary against a
malicious X11 client spoofing properties. No broad root crop or sibling inference
is introduced to work around that limitation.

Six focused pure tests cover clipped pixel placement, unchanged canvas/outside
pixels, image-size/alpha rejection, format gates, occlusion intersection and
snapshot changes. Rustfmt and diff checks pass. The local macOS `cargo test`
attempt cannot validate this Linux-gated module and fails existing Linux-only
`kwin_helper_contract` imports; Linux compilation and all real desktop acceptance
remain pending. The Shape query method signature was checked against pinned
[x11rb0.13.2 documentation](https://docs.rs/x11rb/0.13.2/x11rb/protocol/shape/trait.ConnectionExt.html).
No new product or agent pass is claimed by these preparation checks.

Three additional opt-in real-X11 integration tests create owned drawable windows
and use the existing real EWMH activation path. They cover correct visible pixels,
wrong PID, sibling transient, missing ownership, unmapped popup and background
raw behavior; clipping; and actual Shape, opacity, occlusion and between-capture
resize refusal. These are live X-server fixtures, not mocked pixels or protocol
responses. RAII destroys only their owned windows. Run them serially on a dedicated
24-bit Xvfb with a real EWMH window manager, never on a user desktop:

```sh
CUA_POPUP_CAPTURE_DISPOSABLE_X11=1 cargo test -p platform-linux --lib \
  capture::popup::live_tests -- --ignored --test-threads=1
```

The marker acknowledges that activation is fixture setup. It does not bypass
production capture guards. A live application resize between the two real pixel
reads deliberately exercises the readback guard. No such mutation is in
production capture. These integration tests are drafted but unexecuted locally;
Linux compilation, real integration execution and the parent's public SDK
acceptance remain required.

First Linux build34591610618 failed before runtime with Rust E0597 in the
property reader's tail expression. Binding the collected values locally ends the
borrow before the reply is dropped. The failed build is retained; the correction
requires a fresh Linux build and has no accepted runtime result yet.
