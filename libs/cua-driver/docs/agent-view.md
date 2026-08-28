# Agent View

Agent View is an optional, always-on-top miniature desktop that shows the exact
application windows and browser tabs traversed by one Cua Driver agent session.
It is enabled by default on supported interactive desktops. Disable it for one
daemon invocation with:

```console
cua-driver --no-agent-view
```

`--agent-view` remains accepted as an explicit enable for compatibility. The
durable config equivalent is `{"agent_view": false}` in
`~/.cua-driver/config.json`.

Use `--agent-view-geometry WxH[+X+Y]` to override its initial size and optional
top-left position. The default is `640x420` near the top-right of the main
display. The view is resizable on macOS, Windows, and Linux X11/XWayland.
Pure Wayland uses a native top-right layer surface when the compositor exposes
`zwlr_layer_shell_v1`; its position and size are compositor-managed. GNOME
sessions without layer-shell use the supported XWayland presentation path. A
pure layer-shell view remains click-through and follows the most-recent session;
local tab selection, dragging, and manual resizing require the X11/XWayland
window path because standard Wayland does not expose those global window
management operations to clients.

## Session model

- One Agent View window belongs to one daemon and displays one selected private
  runtime session at a time. Multiple sessions appear as local tabs in that
  same window; the tab strip is hidden when only one session has cards.
- Until a person selects a session locally, Agent View follows the session with
  the most recent exact target activity.
- When multiple sessions have cards, the native Agent View window exposes a
  local session switcher. Selecting a session pins the view there until that
  session ends or is removed. There is no agent-facing selection tool.
- Public session names are display labels only. The private runtime session ID
  remains the isolation identity, so equal labels never combine sessions.
- Ending, disconnecting, revoking, or idle-expiring a session removes its cards.
  Reusing a revived session starts with an empty view.

Agent View does not claim targets and does not add claim/release semantics to
the CLI, MCP tools, SDKs, or target schemas. Switching the presentation never
focuses, moves, resizes, closes, or otherwise controls an underlying target.
During physical Cua Driver actions, the always-on-top surface temporarily
becomes input-transparent so an overlapping Agent View cannot intercept input
intended for the represented application. Local switching and resizing resume
as soon as the action finishes.

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
and exact target arguments. This session-oriented behavior does not introduce
new MCP tools, CLI flags, SDK types, capability IDs, claim tokens, or public
target/session fields.
