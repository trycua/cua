# Wayland Compositor Identity Plan

This is an architecture explanation and evidence plan for expanding exact
browser attachment beyond Sway. It does not claim generic Wayland support.

## Why Compositor Identity Is Required

Existing-profile setup can open an internal browser page and press one exact
accessibility control. Before doing so, Cua Driver must prove that the native
toplevel, browser PID, geometry, AT-SPI application/window, product descriptor,
and loopback endpoint all describe the same browser generation.

Title, application ID, or screen-coordinate similarity alone is insufficient.
An attacker or unrelated window can reuse those values. When the compositor
cannot attest exact identity, mutation must refuse before opening the setup
page or sending input.

## Accepted Sway Route

The existing Sway adapter uses compositor IPC to correlate the exact container,
PID, and rectangle with the AT-SPI tree. Setup may briefly focus that attested
container for fixed browser-owned internal navigation, then restores the prior
container. Canonical Sway evidence accepts Google Chrome, Chromium, and
Microsoft Edge after each product completed the same full matrix with playable
video and no unexpected results.

## Accepted GNOME Route

The maintained WinRects extension v4 exposes the narrow identity route Cua
Driver needs on GNOME Shell and Mutter: a stable window identifier, PID,
geometry, focus and stacking state, and exact activation.

The Linux adapter resolves the extension's immutable unique D-Bus owner,
verifies that the owner is the current user's system-installed `gnome-shell`
process, requires helper API v4 for browser-sensitive calls, and addresses that
unique owner directly. This prevents an unrelated same-session process from
replacing the public bus name between verification and activation. Setup and
consent briefly activate one exact window, then restore and verify the prior
Shell focus.

The route is accepted for Google Chrome after the full standalone-browser
matrix recorded 13 delivered behaviors, 3 policy refusals, 0 failures, and 0
skips with 16 playable videos at one source commit. Other Chromium-family
products still require their own product-specific evidence on GNOME.

The adapter refuses when the component is missing, incompatible, disabled, or
unable to distinguish multiple windows for one browser process. Enabling a
generic unsafe-mode switch is not an acceptable production dependency.

## Candidate Compositors

### KDE Plasma and KWin

KWin 6 scripting exposes a per-window internal ID, PID, frame geometry,
stacking order, active state, and workspace activation. See the official
[KWin scripting API](https://develop.kde.org/docs/plasma/kwin/api/).

A maintained Cua Driver KWin component could return a versioned, read-only
identity record and perform one exact focus-and-restore transition. The adapter
must bind its D-Bus peer, KWin generation, internal window ID, PID, geometry,
and AT-SPI window. A generic user-provided script or title lookup is not enough.

### Other Compositors

The standard `ext-foreign-toplevel-list-v1` protocol exposes an opaque stable
toplevel identifier, title, and application ID, but not PID or geometry. See
the [foreign toplevel protocol](https://wayland.app/protocols/ext-foreign-toplevel-list-v1).
It can contribute one part of a proof, but cannot independently authorize
mutation.

wlroots compositors may expose additional management or compositor IPC. Each
route remains compositor-specific until it proves all required fields. No
screen-coordinate fallback is allowed.

## Threat Cases

Each adapter and harness must cover:

- two same-title browser windows with equal or near-equal geometry;
- PID reuse or browser restart between approval and mutation;
- a decoy application with the same application ID or title;
- a window moving, changing workspace, minimizing, or closing during setup;
- stale compositor object IDs after remap;
- an incomplete or mismatched AT-SPI tree;
- endpoint ownership or generation drift; and
- focus restoration failure after a bounded setup action.

Any ambiguity produces a stable, non-mutating structured refusal.

## Release Acceptance Per Compositor

A compositor is listed as supported only after the canonical matrix records:

1. exact compositor, distribution, browser product, and version provenance;
2. setup, attach, reconnect, multi-tab, stale-ref, background type, trusted
   input policy, and ambiguous-window rows;
3. external page-state oracles rather than driver success responses;
4. continuous focus, z-order or occlusion, and no-leaked-input guards;
5. cursor preservation where the compositor exposes a trustworthy readback;
6. before and after evidence plus playable video at the exact source commit;
7. explicit environment results for missing APIs or accessibility trees; and
8. generic and unknown Wayland sessions refusing before mutation.
