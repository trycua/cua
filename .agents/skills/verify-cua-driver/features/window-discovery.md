# Window discovery and state

Discover an app window, preserve its process/window identity, and obtain an accessibility snapshot before sending input.

## Sub-features

- `list-windows`: enumerate mapped windows and their identities.
- `select-window`: distinguish the new fixture from existing windows and the sentinel.
- `window-state`: obtain AX content and snapshot-bound element tokens.
- `readiness`: require both AX and independent journal readiness.

## How to get to it (user POV)

Call MCP `list_windows` followed by `get_window_state`, or use those tool names through the CLI `call` surface. The typed SDK exposes native-window discovery and state as well.

## Driving it with MCP/testkit

Preconditions: [Launch and Doctor](../SKILL.md) have a usable display and accessibility bus.

- **Discover and inspect:** run `timeout 1260 .agents/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build`. Preflight and both click cells enumerate windows before launching, then select a newly appearing `CuaTestHarness Electron` title and retain its actual PID/window ID.
- **AX:** inspect `tool-calls.jsonl` for `list_windows` with `{}` and `get_window_state` with the discovered `pid`, `window_id`, `capture_mode: "ax"` and testkit's `full_output`. Require `WEB_HARNESS_MARKER_v1` before the `READY` signal and the action.
- **Identity:** the SDK sibling must preserve the fixture PID/window identity on post-action state and mint a new snapshot-bound token. Require typed stale/mismatched handle refusals, not fallback to another window.
- **Proof:** retain discovered identities, AX tree, transcript and captured images. CLI discovery and broader desktop-state entry points remain unverified by this recipe.

## Gotchas

Do not identify a window solely by a guessed PID, title or socket presence. Fixtures can expose a window before their AX subtree is usable. A fresh snapshot invalidates old tokens. X11 window IDs, Wayland compositor handles and Windows handles are not interchangeable.
