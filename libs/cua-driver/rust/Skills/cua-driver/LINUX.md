# cua-driver — Linux

Start with `cua-driver doctor` in the graphical user's session. X11,
XWayland, and native Wayland expose different capture and input facilities;
successful discovery does not prove either input delivery or window capture.
Use [WORKFLOW.md](WORKFLOW.md) for exact targets and verification and
[RUNTIME.md](RUNTIME.md) for transport and permission ownership.

On X11, `set_window_frame({pid, window_id, x, y, width, height})` sends an
EWMH window-manager request and confirms it against `list_windows` geometry.
Wayland has no portable protocol for setting another application's top-level
geometry, so this tool refuses there unless a future compositor-owned adapter
can provide exact targeting and readback.

AT-SPI is talked to natively over D-Bus (the `atspi`/zbus crate) — no
`pyatspi` or GObject-introspection typelibs are required at runtime.

## Delivery

Background window input must not activate or raise the target. AT-SPI actions
can reach semantic controls without raw pointer delivery; X11 pixel dispatch
can use AT-SPI hit-testing or a virtual-pointer route. Do not assume all input
uses XSendEvent or that every toolkit accepts it. Inspect the result's public
route and independently verify the application.

`delivery_mode:"foreground"` is a user-visible takeover. Never select it automatically.
Use explicitly authorized `delivery_mode:"foreground"` only after evidence
that the chosen background route is unavailable or ineffective. Target-specific
activation and restoration depend on the compositor adapter. If activation
cannot be proven, accept the refusal instead of sending keys to whatever is focused.

The overlay is separate from the physical pointer; cursor-bearing and
keyboard actions re-show it automatically. Distinct overlays do not isolate
desktop input. Keep one controller for a global-input workflow.

## Capture recovery

`get_window_state` requests both AT-SPI and a window screenshot by default.
A valid image is not guaranteed. If `screenshot_error.code` is
`surface_identity_unproven`, the driver cannot attest that output pixels
belong to the requested surface. Preserve that distinction from an empty or
untrusted accessibility tree. Never crop a desktop image and label it an
attested window capture.

1. Read any returned tree and `degraded_reason`; semantic interaction may still
   be possible without pixels.
2. Do not retry a window-pixel action without a valid window image. Changing
   `capture_mode`, inventing bounds, or repeatedly increasing timeouts cannot
   establish surface identity.
3. If the task permits full-display capture and visible desktop control,
   explain the broader scope and obtain authorization if not already given.
   Use `get_desktop_state`, then
   `target:{"kind":"desktop","display_id":"primary"}` on input, and verify
   through another desktop snapshot. See [the desktop loop](WORKFLOW.md#desktop-loop).
4. If desktop scope is not admitted, report the limitation and stop that route.

A capture request may be waiting for an OS/portal permission dialog. Inspect
window discovery for a pending prompt, or ask the user to check the desktop.
Let the user approve or deny it. Do not repeatedly start captures, automate
approval, or change security settings as a workaround. After approval, take a
fresh capture and verify its dimensions/content before continuing. A timeout
alone does not prove which permission or backend failed.

Supporting smoke evidence: Driver 0.23.2 on KDE/KWin Wayland opened an Electron
music app, searched, selected an album track, and showed playback through this
desktop route after user screenshot approval. The window route returned
`surface_identity_unproven`; desktop actions returned `global_input` and
`unverifiable`, so screenshots and user confirmation established the result.
This is not certification of background targeting, other compositors, or video.

## Native application menus

Use `invoke_menu({pid, window_id, path:[...]})` for a known GTK/Qt application
menu command. It activates the exact target only for the duration of the
operation, resolves each labelled AT-SPI menu descendant again after the prior
menu expands, and refuses missing, duplicate, disabled, or non-actionable
segments. It works through AT-SPI on both X11 and Wayland and never falls back
to coordinates. Verify the command's semantic effect from fresh state; the
native `do_action` acknowledgement alone is not task completion.

## AT-SPI needs the session bus (headless / containers / `runuser`)

AT-SPI — the accessibility tree behind `get_window_state`, element-indexed
clicks, and focus-free `type_text` — lives **entirely on the desktop
session's D-Bus**. cua-driver reaches it via `DBUS_SESSION_BUS_ADDRESS`. When
the daemon is started _inside_ a normal desktop login that variable is already
exported and everything works. When it is started **outside** the session —
a container entrypoint, a headless box, `runuser`/`su` into the desktop user,
a systemd _system_ unit, or a VNC session running its own ad-hoc bus — the
variable is unset, the AT-SPI registry walk comes back empty, and
`get_window_state` reports **every** window as having no elements.

cua-driver now **auto-discovers the session bus at startup** (mirroring the
`XAUTHORITY` recovery): if `DBUS_SESSION_BUS_ADDRESS` is unset it adopts
`/run/user/<uid>/bus`, or reads the address out of a running desktop-session
process's `/proc/<pid>/environ` (`xfce4-session`, `gnome-session`, …). So the
common headless cases now "just work". The two things that still must be true:

1. **An accessibility bus must be running** in that session, and
   **`toolkit-accessibility` must be on** — cua-driver advertises a screen
   reader at startup to flip it, but a session with no a11y bus at all
   (`/usr/libexec/at-spi-bus-launcher`) can't expose a tree. `cua-driver
doctor` now probes `org.a11y.Bus` for real (not just "is there a bus?")
   and tells you which of the two is missing.
2. The daemon must run **as the desktop user** (so it can read that user's
   session-process environ and the `/run/user/<uid>/bus` socket). Running the
   daemon as root against a user session is the Linux analogue of the Windows
   "Session 0" isolation problem.

An empty AT-SPI walk is now surfaced honestly: `get_window_state` sets
`degraded: true` + a `degraded_reason` (instead of a bare `elements: []`) so a
caller can tell "this window genuinely has no controls" apart from "the a11y
bridge isn't up / the daemon isn't on the session bus".

## The validated modality matrix (X11 / XFCE)

Each input rung and its stable public route:

| Modality                        | `delivery_mode`           | `route`                                                              | Postcondition proof                                      |
| ------------------------------- | ------------------------- | -------------------------------------------------------------------- | -------------------------------------------------------- |
| Element click (`element_index`) | `background`              | `accessibility`                                                      | Use `verify_state`; invocation alone is not confirmation |
| **element px action (x,y)**     | `background`              | `accessibility` when AT-SPI-at-point lands, otherwise `global_input` | Use `verify_state` or multimodal reading                 |
| Pixel (px) click, escalated     | `foreground`              | `global_input`                                                       | Use `verify_state` or multimodal reading                 |
| `type_text` into editable       | `background`              | `accessibility`                                                      | `confirmed` only with `value_readback` evidence          |
| `type_text`, non-editable focus | `background`/`foreground` | `synthetic_events` or `global_input`                                 | Use `verify_state` or multimodal reading                 |

**A background element px action does land on X11** — for an AX-exposing app it
takes the focus-free AT-SPI `do_action`-at-point path (`x11_atspi`), exactly
like the macOS/Windows background pixel click. It falls to the MPX
virtual-pointer path (`x11_pixel`) only for non-AX surfaces, **and that path
needs a real Xorg + `/dev/uinput`** — under Xvnc / minimal containers without
uinput, escalate to `delivery_mode:"foreground"`. (`type_text` in the
`background` rung is focus-dependent for non-editable widgets; that's the one
genuine background limitation, and `foreground` is the documented escalation.)

## Wayland

Set `CUA_DRIVER_RS_ENABLE_WAYLAND=1` to enable native Wayland support. The
driver selects a backend from compositor capabilities:

- Sway and other wlroots compositors use foreign-toplevel discovery,
  wlr-screencopy, virtual pointer, and virtual keyboard protocols.
- Hyprland has separate discovery and capture adapters. Its optional plugin
  defaults to discovery-only; the opt-in input v3 source candidate has the
  qualification and validation limits below. Do not inherit Sway coverage.
- GNOME/Mutter uses the bundled WinRects Shell helper for target geometry and
  activation, plus portal/libei for foreground raw input.
- KDE/KWin uses AT-SPI and portal facilities where available. Target-specific
  foreground activation remains experimental, so unsafe raw input refuses.
- The optional `cua-compositor` is a separate nested session enabled
  explicitly for controlled automation. GNOME and KDE never switch into it.

Sway recording works through the wlroots recorder path and is exercised by the
canonical harness runner. Portal-backed GNOME recording is still an evidence
gap. Capture and recording availability therefore depend on the compositor,
installed helpers, and portal grant.

Standard Wayland has no general client protocol for raw input to an arbitrary
occluded surface. Background AX actions can still deliver through AT-SPI, and
a PX left click can deliver when hit-testing resolves to an actionable AT-SPI
control. Other focus-bound background pointer and keyboard shapes return an
exact `background_unavailable` result. They do not report success after a
silent drop.

Outside an explicitly enabled, qualified compositor-owned background route,
raw Wayland input requires explicitly authorized `delivery_mode:"foreground"`.
The driver activates the selected target through a verified compositor adapter
before dispatch. If
the compositor has no target-addressable activation or input backend, the call
refuses before sending input. Reconstructing coordinates alone does not make
raw background PX possible on a standard compositor.

### Hyprland input v3 source candidate

[PR #3572](https://github.com/trycua/cua/pull/3572) records dated, exact-source
validation results for the experimental opt-in input v3 candidate. Acceptance
requires the unchanged complete Linux canonical runner on native Hyprland and
separate bounded qualified-app proof. The default plugin build remains
discovery-only.
The candidate source reports Driver `0.23.2`; published Driver `0.23.2` does not
include these branch changes. Switching Driver channels does not install or
enable the plugin. Build and loading require the exact Hyprland ABI and compiler
toolchain; v3 uses `CUA_HYPRLAND_INPUT=ON`, separate from the historical
`CUA_HYPRLAND_TEST_INPUT` experiment.

Driver admits each action through its normal shared permission, resource, and
lifecycle policy. There is no additional Omarchy approval panel or external
signer. The plugin accepts the trusted desktop account over same-user local
sockets; this does not sandbox native code running as that user. Application
qualification is a compatibility check, not authorization.

The initial native qualification scope is Calc from `libreoffice-fresh 26.2.5-3`
and Inkscape `1.4.4-6`, subject to per-operation native evidence. Before each
action, Driver matches `/proc/<pid>/exe` to the canonical executable path
(`/usr/lib/libreoffice/program/soffice.bin` or `/usr/bin/inkscape`), checks the
exact package name and version in the local pacman database and its executable
file listing, and rechecks process identity. Unknown or unavailable package
identity refuses. Package eligibility does not certify every LibreOffice
application or operation.

The plugin separately binds the exact live native surface and checks geometry,
desktop availability, primary-client and other-lane conflicts, and the compiled
default `evdev`/`pc105`/`us` keymap. The candidate excludes variants, options,
remaps, multiple layout groups, missing keyboards, Unicode, IME input,
arbitrary held-key streams, and modified pointer gestures. Chromium, Electron,
and XWayland raw background input are outside this scope. AT-SPI routes retain
their separate behavior.

Two compositor seats, `Cua-Agent` and `Cua-Agent-2`, persist across configuration
disable/re-enable. Each connection claims one lane, and each admitted action
requires a fresh target binding. Plugin replacement requires a desktop restart;
do not treat historical experiment reload workarounds as a supported lifecycle.
Refusals never authorize a hidden foreground fallback, display wake, or session
unlock. A dispatch acknowledgement is `effect:"unverifiable"`; verify the
application effect from fresh state. Do not replay canceled, partial, or unknown
actions.

The candidate also adds an explicitly requested foreground route, advertised
by the plugin as `foreground_target:true`. It binds the exact native top-level
surface on the compositor thread and intentionally changes primary focus and,
for pointer actions, cursor position. It does not restore the previous focus or
cursor. This route has no Calc/Inkscape background package gate. The canonical
native harness covers defined GTK3, Electron, and Tauri foreground cases. It refuses
held physical input, grabs, constraints, drag-and-drop, ambiguous primary seat
bindings, and non-neutral keyboard modifiers. Background refusal never selects
this route automatically. Driver expands bounded ASCII text under the exact
US keymap; Unicode and IME remain outside its raw-input scope.

The retained bounded app evidence at source
`f180e8828b8f31cc153e3c44eaa89a9c13c5bc68` includes instrumented Calc/Inkscape
proof on both seats and an uninstrumented smoke. The plugin tree and
uninstrumented module hash are unchanged at
`1133a06e4f205cf80188a7ac9e41102f37611fea`. The proof covers recorded actions
and observation intervals, not every application operation or release package.
Portable tests and historical experiments do not replace complete native
harness acceptance. Compatible release artifacts and final Fleet image
packaging and lifecycle validation require separate evidence. Physical Omarchy
parity requires separate acceptance; it is not a gate for publishing a validated
Fleet image.

## Quick triage

If a tool call surprises you on Linux:

1. `cua-driver doctor` — reports the display server (X11 / Wayland),
   **whether `org.a11y.Bus` actually answers on the session bus** (not just
   "is there a bus"), the discovered `DBUS_SESSION_BUS_ADDRESS`, and
   `ffmpeg` availability (for recording).
2. Check `XDG_SESSION_TYPE` — X11 still has toolkit-specific delivery limits; `wayland`
   needs `CUA_DRIVER_RS_ENABLE_WAYLAND=1` for the native backend,
   else XWayland.
3. **Empty AT-SPI tree** (`get_window_state` returns `degraded:true`) — in
   order of likelihood: (a) the daemon isn't on the desktop session bus
   (headless / container / `runuser` / root-against-user-session — see
   _AT-SPI needs the session bus_ above; doctor will say
   `DBUS_SESSION_BUS_ADDRESS unset`); (b) the a11y bridge is off
   (`gsettings set org.gnome.desktop.interface toolkit-accessibility true`);
   (c) GTK4 / Qt6 / Chromium populate lazily — re-snapshot after an
   interaction or an AX-enable settle.

## Forbidden vectors

Same idea as macOS / Windows — don't shell out to anything that
foregrounds a target:

- `wmctrl -a <window>` / `wmctrl -R <window>` — activates / raises.
- `xdotool windowactivate <wid>` — activates.
- `xdotool key --window <wid> alt+Tab` — focus churn.

Prefer cua-driver tools with an explicit `window_id`. When in doubt,
ask the user.

## What to expect

| Environment             | Proven baseline                                                                                                                | Main limits                                                                                                                                                                                                    |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| X11/Openbox             | AT-SPI trees and actions, foreground pointer and keyboard input, window and desktop capture, and video                         | Raw background delivery remains toolkit-specific; unsupported shapes refuse                                                                                                                                    |
| Sway/wlroots            | AT-SPI, native discovery, full-display and cropped-window screencopy, foreground input, semantic background actions, and video | Raw background pointer and keyboard input remains focus-bound                                                                                                                                                  |
| Hyprland/Omarchy        | Experimental source candidate with separate discovery-foundation and bounded two-seat app evidence                             | Default plugin is discovery-only; raw background v3 qualification is limited to the exact native Calc/Inkscape packages and plain US keymap; complete native harness and release acceptance are separate gates |
| GNOME/Mutter            | AT-SPI, WinRects geometry and activation, capture, and portal/libei foreground input                                           | Requires the helper and portal grant; portal video parity remains open                                                                                                                                         |
| KDE/KWin                | AT-SPI and generic discovery where exposed                                                                                     | Target-specific activation and behavioral coverage remain experimental                                                                                                                                         |
| Nested `cua-compositor` | Versioned direct per-surface input, native GTK 31/31, capture/scope 5/5, and partial Electron coverage                         | The complete shared matrix remains experimental; do not infer standard-Wayland support                                                                                                                         |

See [WORKFLOW.md](WORKFLOW.md) for exact targeting and verification and `RECORDING.md` for session
recording.
