---
name: cua-driver-rs-linux
description: Drive a native Linux app via the cua-driver CLI — snapshot the AT-SPI tree, click by element_index or pixel, type, screenshot, record, verify via re-snapshot. Background by default (no foreground steal, no real-pointer move) on X11. Wayland input/capture is opt-in and still preview.
---

# cua-driver-rs — Linux

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
→ behavior matrix. The Linux-specific value of `recommended` is **`foreground`
on Wayland** (background pixel can't target an unfocused window) and
`vision_pixel` on X11 (a `vision` capture + background pixel click lands via
AT-SPI `do_action`-at-point — the matrix below).

## `capture_mode` — two modes, 1:1 with the action path

The capture vocabulary is the same two modes as macOS/Windows, and each maps
to one addressing mode: **`ax`** (DEFAULT) returns the AT-SPI tree and **no
image** — act by `element_index`; **`vision`** returns a screenshot and **no
tree** — act by pixel `x,y`. Picking `vision` is a deliberate switch to the
pixel path. `get_window_state` returning `degraded:true` (empty AT-SPI walk)
is the cue to cross to `vision` (X11) or escalate to `delivery_mode:"foreground"`
(Wayland). (`som` still decodes as a deprecated alias for `ax`; it no longer
means "tree + screenshot".)

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
| **Pixel / vision click** | `background` | `x11_atspi` (AT-SPI `do_action`-at-point) for AX apps; else MPX `x11_pixel` | ✅ when AT-SPI-at-point lands; else best-effort |
| Pixel click (escalated) | `foreground` | `x11_pixel_fg` (EWMH activate → inject → restore) | ❌ confirm via screenshot |
| `type_text` into editable | `background` | `ax` (AT-SPI `insertText`) | ✅ `verified:true` |
| `type_text`, non-editable focus | `background`/`foreground` | `key_events` / `key_events_fg` | ❌ confirm via screenshot |

**Background pixel/vision click does land on X11** — for an AX-exposing app it
takes the focus-free AT-SPI `do_action`-at-point path (`x11_atspi`), exactly
like the macOS/Windows background pixel click. It falls to the MPX
virtual-pointer path (`x11_pixel`) only for non-AX surfaces, **and that path
needs a real Xorg + `/dev/uinput`** — under Xvnc / minimal containers without
uinput, escalate to `delivery_mode:"foreground"`. (`type_text` in the
`background` rung is focus-dependent for non-editable widgets; that's the one
genuine background limitation, and `foreground` is the documented escalation.)

## Wayland (opt-in — still preview)

Native Wayland support is behind an opt-in flag and not yet at the same
maturity as X11:

- `CUA_DRIVER_RS_ENABLE_WAYLAND=1` enables the native-Wayland backend.
  On wlroots compositors (labwc/sway) it uses foreign-toplevel +
  screencopy; on GNOME-Mutter / KDE-KWin, which expose no client
  protocols for cross-window enumeration, cua-driver brings its own
  nested compositor (`CUA_WAYLAND_NEST=1`).
- Without the flag, on a Wayland session the driver falls back to
  XWayland where available.
- Screenshots work via `grim` / wlr-screencopy. **Video recording is
  not yet available on Wayland** — the recorder is `x11grab`, which is
  X11-only.
- **`delivery_mode` on Wayland is constrained by the protocol**, honestly:
  input goes through libei + xdg-desktop-portal, which injects to the
  **compositor's input focus** — there is no per-window background targeting
  like X11/macOS/Windows. So `background` can't aim at a specific non-focused
  window, and `bring_to_front` has no standalone external activate (the
  compositor bundles activation into the virtual-pointer/click path) — it
  returns `bring_to_front_wayland_bundled` and points you at
  `delivery_mode:"foreground"` on the input call instead. The delivery
  contract also defines a structured `background_unavailable` error for the
  no-libei-backend case (built without `portal-libei` or a denied portal
  session); when input has no actuator the tools surface an error rather
  than silently succeeding.
- **Wayland escalation is `foreground`, NOT `vision_pixel`.** This is the
  one place the cross-platform escalation hint flips. Everywhere else
  (macOS, X11, most Windows surfaces) a background `vision` pixel click can
  land on an unfocused window, so `escalation.recommended` is `vision_pixel`.
  On Wayland an unfocused window **cannot** be pixel-targeted in the
  background (libei injects only to compositor focus → `background_unavailable`),
  so the hint on action responses and on a `degraded` `get_window_state` is
  `foreground`: re-call the action with `delivery_mode:"foreground"`, don't
  drop to a pixel capture expecting a background click to land.

## Quick triage

If a tool call surprises you on Linux:

1. `cua-driver doctor` — reports the display server (X11 / Wayland),
   **whether `org.a11y.Bus` actually answers on the session bus** (not just
   "is there a bus"), the discovered `DBUS_SESSION_BUS_ADDRESS`, and
   `ffmpeg` availability (for recording).
2. Check `XDG_SESSION_TYPE` — `x11` is fully supported; `wayland`
   needs `CUA_DRIVER_RS_ENABLE_WAYLAND=1` for native input (preview),
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

| Intent | Status |
|---|---|
| Snapshot AT-SPI tree | ✅ GTK3/4, Qt5/6, wxWidgets, Electron (GTK4/Qt6 can be partial — re-snapshot) |
| Pixel click | ✅ background `XSendEvent`, no focus steal, no pointer move |
| Element-indexed click | ✅ AT-SPI `do_action` |
| Type text | ✅ AT-SPI focus-free, with XTest / pty fallback for the focused widget |
| Hotkey / `press_key` | ✅ |
| Screenshot full-display | ✅ X11; ✅ Wayland via `grim` |
| Screenshot per-window | ✅ X11 |
| `launch_app` | ✅ env-scrubbed launch (no workspace steal) |
| Recording (video) | ✅ X11 (`x11grab`); ⚠️ Wayland not yet supported (preview) |

See `SKILL.md` for the cross-platform loop (snapshot-before-AND-after,
pixel-click contract, failure modes) and `RECORDING.md` for session
recording.
