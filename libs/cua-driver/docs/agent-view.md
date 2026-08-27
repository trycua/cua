# Agent View

Agent View is an optional, always-on-top miniature desktop that shows the exact
application windows and browser tabs traversed by one Cua Driver agent session.
Enable it when starting the daemon:

```console
cua-driver --agent-view
```

Use `--agent-view-geometry WxH[+X+Y]` to override its initial size and optional
top-left position. The default is `640x420` near the top-right of the main
display. The view is resizable on macOS, Windows, and Linux X11/XWayland.
On macOS, the local **Expand** button grows Agent View into a large centered
viewer without activating or resizing the represented application. Click
**Shrink** to return immediately, or leave it alone and Agent View returns to
the prior compact frame after eight seconds.

## Session model

- One Agent View window belongs to one daemon and displays one selected private
  runtime session at a time.
- Until a person selects a session locally, Agent View follows the session with
  the most recent exact target activity.
- When multiple sessions have cards, the native Agent View window exposes a
  local session switcher. Selecting a session pins the view there until that
  session ends or is removed. There is no agent-facing selection tool.
- Public session names are display labels only. The private runtime session ID
  remains the isolation identity, so equal labels never combine sessions.
- Cards keep a stable slot within the selected session when their contents
  refresh, so activity in one target does not make neighboring cards swap.
- Ending, disconnecting, revoking, or idle-expiring a session removes its cards.
  Reusing a revived session starts with an empty view.

Agent View does not claim targets and does not add claim/release semantics to
the CLI, MCP tools, SDKs, or target schemas. Switching the presentation never
focuses, moves, resizes, closes, or otherwise controls an underlying target.
During ordinary physical Cua Driver actions, the always-on-top surface
temporarily becomes input-transparent so an overlapping Agent View cannot
intercept input intended for the represented application. Background-only mode
does not use that behavior: its admitted input is exact and semantic, so Agent
View keeps its local controls clickable throughout the action.

## macOS background-only mode

For a same-login-session view that refuses actions known to disturb the user's
foreground desktop, start the daemon with:

```console
open -n -g -a CuaDriver --args serve --agent-view-background-only
```

This flag implies `--agent-view`. It does not create or connect to a VM, switch
macOS users, or start another WindowServer session. Agent View remains one
non-activating PiP window in the current login session. The window cards and
cursor inside it are presentation: cards are exact target captures, and the
cursor is the last agent action position drawn over its target.

Complete the trusted capture setup before using this mode:

```console
cua-driver permissions grant
```

That command requests **Accessibility**, **Screen Recording**, and direct
ScreenCaptureKit access through CuaDriver.app. It then asks ScreenCaptureKit for
one native 2-by-2 display frame, checks that the frame completed without a
fallback, wipes those 16 pixel bytes in memory, and does not write them to a
file. macOS can show its own capture acknowledgement during this trusted setup,
so finish it before starting background-only work. Strict startup only checks
the existing TCC grants. If either base grant is missing, it exits with setup
instructions instead of showing a prompt or opening System Settings over the
user's desktop.

macOS may renew its capture acknowledgement later. That system UI can briefly
change the active/key application. Background-only mode cannot suppress it, so
cold or renewed first-capture non-disruption is not yet guaranteed.

The mode changes dispatch behavior on macOS:

- native mutation is limited to a semantic accessibility press with an exact
  `pid`, `window_id`, fresh element token (or snapshot-bound element index),
  and explicit `delivery_mode: "background"`;
- raw coordinates, synthetic mouse/keyboard fallbacks, shared clipboard
  writes, and legacy `page` mutations are refused;
- browser mutation is currently limited to typed click and text actions on an
  exact, session-owned page in a pre-opened embedded Chromium application;
- applications and browser endpoints must already be open and available,
  because launching or preparing one can activate or relaunch main-session UI;
- normal already-open Chrome profiles are not yet supported in this posture,
  because the setup step that grants access is intentionally refused;
- the AppKit panel is built synchronously on the main thread, remains
  non-activating by construction, and daemon startup fails if the panel cannot
  be created and made visible;
- the process-global cursor overlay is disabled, while the last agent action
  position continues to render as the synthetic cursor inside Agent View;
  calls that reconfigure that global overlay are also refused;
- `delivery_mode: "foreground"` is refused before authorization, recording,
  cursor animation, or platform input;
- every modifying tool is refused by default. The small semantic native and
  embedded-browser shapes above, session bookkeeping, and non-prompting
  permission status are the only current exceptions;
- Cua Driver's own process is refused as an input target, so the agent cannot
  click or type into its observer window; and
- local Expand/Shrink remains clickable while the agent works, and an expanded
  glance automatically collapses after eight seconds.

This is deliberately narrower than full macOS session isolation. Applications,
the pasteboard, notifications, and other per-user resources still belong to the
same login session. An allowed background action can also change its target's
content or cause that application to create UI. The contract is therefore
"background-only certified-target input," not a second desktop, VM, or
security boundary. Use ordinary Agent View when foreground fallbacks and
window/app management are required.

## What becomes a card

A target is added or refreshed only after Cua Driver receives exact target
evidence through one of these paths:

- `get_window_state` with both `pid` and `window_id`;
- an exact native action carrying both `pid` and `window_id`;
- `get_browser_state` with both `target_id` and `tab_id`; or
- an exact browser-tab action carrying both `target_id` and `tab_id`.

Broad discovery such as `list_windows`, `list_browser_tabs`, whole-desktop
snapshots, ambient foreground activity, and actions performed outside Cua
Driver do not populate Agent View.

Browser cards use the page's stable internal CDP identity, so navigation and a
repeated browser-window bind refresh the same card. Multiple tabs in one Chrome
or Edge window remain distinct, while their native container window is hidden
to avoid duplicating the browser.

When a target is proven absent, only its exact card is removed. When a session
ends, all cards belonging to that private session are removed. The underlying
application or tab is never changed by presentation cleanup.

## Public contract

Agent View uses the existing daemon flag, geometry option, runtime sessions,
background-only mode, and exact target arguments. This session-oriented
behavior does not introduce new MCP tools, SDK types, capability IDs, claim
tokens, or public target/session fields.
