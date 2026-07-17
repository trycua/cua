# cua-driver — Linux

The Linux backend drives X11 apps **in the background**: clicks and
keystrokes are injected to the target window without raising it,
activating it, or moving the real pointer — the same no-foreground
contract the macOS and Windows backends hold. The full tool surface is
supported: `click`, `type_text`, scroll, `press_key`, `screenshot`,
`launch_app`, `list_apps`, `list_windows`, `get_window_state`, and
session recording.

AT-SPI is talked to natively over D-Bus (the `atspi`/zbus crate) — no
`pyatspi` or GObject-introspection typelibs are required at runtime.

## How input is delivered (the no-foreground contract)

- **Pixel click** — `XSendEvent(ButtonPress/Release)` to the resolved
  target window. No raise, no activate, no real-pointer warp. (It does
  **not** use `XTestFakeButtonEvent`, which would route through the
  focused window.)
- **Element click** (`element_index`) — AT-SPI `do_action` on the
  accessible. Toolkit-native, focus-free.
- **type_text** — AT-SPI EditableText first (focus-free; lands in an
  *unfocused* window's editable for Qt6 / GTK4). When a **non-editable**
  widget holds focus — a spreadsheet cell, a terminal, a canvas — it
  synth-types into the focused widget via XTest, and terminals take a
  focus-free pty-injection path. So background typing into an editable
  needs no focus; the XTest path is the foreground "type where I
  clicked" case.
- The **agent cursor** is a synthetic overlay showing where the run is
  acting; it never moves the real pointer (same model as macOS/Windows).
  It glides on clicks and `move_cursor`; issue `move_cursor` to make it
  track a field while typing advances focus across cells.

## `delivery_mode` — the background/foreground ladder

Every input tool (`click`, `type_text`, `press_key`, `hotkey`,
`double_click`, `right_click`, `scroll`) takes an optional **`delivery_mode`**
— the per-call rung of the best-effort-background ladder, matching the macOS
and Windows surface:

- **`background`** (default) — inject **without activating or raising** the
  target. X11: the no-focus-steal paths above (AT-SPI / `XSendEvent` /
  XInput2 MPX pointer). This is cua-driver's differentiator and the right
  default.
- **`foreground`** — **activate the target first** (X11 EWMH
  `_NET_ACTIVE_WINDOW`, proper timestamp handling to beat the WM's
  focus-stealing prevention), inject, then **restore the prior active
  window**. The explicit escalation when a background inject didn't land —
  e.g. a GTK dialog button or a widget that only reads input while focused.
  A brief focus swap unless the target was already active.

**`bring_to_front`** (X11): persistent `_NET_ACTIVE_WINDOW` activation (the
`wmctrl -a` equivalent), kept active. Call it before `delivery_mode:"foreground"`
input to avoid a per-call flash, or to escalate when background didn't land.

**Read-back / `verified`** — `type_text` reports `{verified}`: the AT-SPI
`EditableText.insertText` path (`path:"ax"`) is the **driver-verifiable** rung
(the a11y layer confirms the insert into the widget model) → `verified:true`;
keystroke / XSendEvent / XTest / foreground rungs are not read-back-confirmed
→ `verified:false` (confirm via screenshot). Mirrors the macOS/Windows verdict.

**`effect` / `escalation`** — alongside `verified`, action responses carry the
cross-platform `effect` (`confirmed` / `unverifiable` / `suspected_noop`) and,
when you should change rung, `escalation:{recommended, reason}`. See `SKILL.md`
→ behavior matrix. On a standard Wayland compositor the Linux-specific value
of `recommended` is **`foreground`** (raw background pixels cannot target an
unfocused window); the opt-in nested compositor is a separate environment. Use
**`px` on X11** (an element px action — background pixel click — lands via
AT-SPI `do_action`-at-point off the screenshot already in the snapshot — the
matrix below).

## Perception and the ax/px action choice

`get_window_state` is **perception-mode-agnostic** — by default it returns
**both** the AT-SPI tree **and** a screenshot in one call. You ground on both
and cross-check; the tree **lies** on some surfaces (Electron echo-confirms
`setValue`, virtualized/off-viewport rows report bogus `h:1` frames), so a
grounding screenshot is always present by default. There is no capture mode to
pick.

**Perf opt-out — `include_screenshot`** (boolean, default `true`). Pass
`include_screenshot:false` to skip the screen grab and return tree-only — the
cheap path when you're re-indexing before an **element ax action** and don't
need fresh pixels. It's a **perf** knob, not a modality choice.

**`capture_mode` is DEPRECATED and IGNORED.** It's still *accepted* so old
callers don't error, but it has **no effect** — both the tree and the
screenshot come back regardless of what you pass (`ax`/`vision`/`som`). There
is no `ax`/`vision`/`som` capture choice anymore; drop that vocabulary.

Modality is chosen at **action time**, by how you address the target:

- **element ax action** — `element_index` / `element_token` → AT-SPI
  `do_action`. Backgroundable, driver-verifiable.
- **element px action** — `x,y` → pixel rung, read straight off the screenshot
  already in the `get_window_state` response. Best-effort; caller-confirmed.

`get_window_state` returning `degraded:true` (empty AT-SPI walk) is the cue to
do an **element px action** off that same screenshot (X11) or escalate to
`delivery_mode:"foreground"` (standard Wayland: raw background pixels cannot
target an unfocused window there). The nested compositor has its own
experimental per-surface routes.

## Cross-platform schema residuals (Linux)

The capture/dispatch/addressing params are a shared cross-platform
contract (see `SKILL.md` → *Cross-platform parameter contract*) — the
same `session`, `delivery_mode`, `capture_mode`, `scope`, `modifier`,
`element_index`/`element_token` *shapes* as macOS and Windows, gated in
CI so the three surfaces can't drift. Linux-relevant notes:

- **`session` is now accepted on every action/cursor tool.** Earlier
  Linux builds rejected it via `additionalProperties:false` (it was
  effectively macOS-only); it is now uniformly schema-accepted — Linux
  glides a per-session cursor on X11 where the overlay is available.
- **Windowless screen-absolute clicks** are supported on Linux, but Linux
  has **no per-call `scope` param** (that form is macOS-only). Linux gates
  the windowless path on the persisted `capture_scope` config: opt in once
  with `set_config capture_scope=desktop`, then send `x,y` with no
  pid/window_id. Under the default `capture_scope=window` a windowless
  action is rejected with a structured `desktop_scope_disabled` error.

## AT-SPI needs the session bus (headless / containers / `runuser`)

AT-SPI — the accessibility tree behind `get_window_state`, element-indexed
clicks, and focus-free `type_text` — lives **entirely on the desktop
session's D-Bus**. cua-driver reaches it via `DBUS_SESSION_BUS_ADDRESS`. When
the daemon is started *inside* a normal desktop login that variable is already
exported and everything works. When it is started **outside** the session —
a container entrypoint, a headless box, `runuser`/`su` into the desktop user,
a systemd *system* unit, or a VNC session running its own ad-hoc bus — the
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

Each input rung, and whether the **driver itself** can confirm it (vs. only
the caller agent confirming via screenshot — the same honesty line macOS and
Windows draw):

| Modality | `delivery_mode` | Path reported | Driver-verifiable? |
|---|---|---|---|
| Element click (`element_index`) | `background` | `x11_atspi` (AT-SPI `do_action`) | ✅ a11y action |
| **element px action (x,y)** | `background` | `x11_atspi` (AT-SPI `do_action`-at-point) for AX apps; else MPX `x11_pixel` | ✅ when AT-SPI-at-point lands; else best-effort |
| Pixel (px) click, escalated | `foreground` | `x11_pixel_fg` (EWMH activate → inject → restore) | ❌ confirm via screenshot |
| `type_text` into editable | `background` | `ax` (AT-SPI `insertText`) | ✅ `verified:true` |
| `type_text`, non-editable focus | `background`/`foreground` | `key_events` / `key_events_fg` | ❌ confirm via screenshot |

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

Use `delivery_mode:"foreground"` for raw Wayland input. The driver activates
the selected target through a verified compositor adapter before dispatch. If
the compositor has no target-addressable activation or input backend, the call
refuses before sending input. Reconstructing coordinates alone does not make
raw background PX possible on a standard compositor.

## Quick triage

If a tool call surprises you on Linux:

1. `cua-driver doctor` — reports the display server (X11 / Wayland),
   **whether `org.a11y.Bus` actually answers on the session bus** (not just
   "is there a bus"), the discovered `DBUS_SESSION_BUS_ADDRESS`, and
   `ffmpeg` availability (for recording).
2. Check `XDG_SESSION_TYPE` — `x11` is fully supported; `wayland`
   needs `CUA_DRIVER_RS_ENABLE_WAYLAND=1` for the native backend,
   else XWayland.
3. **Empty AT-SPI tree** (`get_window_state` returns `degraded:true`) — in
   order of likelihood: (a) the daemon isn't on the desktop session bus
   (headless / container / `runuser` / root-against-user-session — see
   *AT-SPI needs the session bus* above; doctor will say
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

| Environment | Proven baseline | Main limits |
|---|---|---|
| X11/Openbox | AT-SPI trees and actions, foreground pointer and keyboard input, window and desktop capture, and video | Raw background delivery remains toolkit-specific; unsupported shapes refuse |
| Sway/wlroots | AT-SPI, native discovery, full-display and cropped-window screencopy, foreground input, semantic background actions, and video | Raw background pointer and keyboard input remains focus-bound |
| GNOME/Mutter | AT-SPI, WinRects geometry and activation, capture, and portal/libei foreground input | Requires the helper and portal grant; portal video parity remains open |
| KDE/KWin | AT-SPI and generic discovery where exposed | Target-specific activation and behavioral coverage remain experimental |
| Nested `cua-compositor` | Versioned direct per-surface input, native GTK 31/31, capture/scope 5/5, and partial Electron coverage | The complete shared matrix remains experimental; do not infer standard-Wayland support |

See `SKILL.md` for the cross-platform loop (snapshot-before-AND-after,
pixel-click contract, failure modes) and `RECORDING.md` for session
recording.
