`cua-driver mcp` serves these tools over stdio. Every tool also runs from the shell as `cua-driver <tool> '<json-args>'` (or `cua-driver call <tool>` through the daemon), with the same code path. Where a tool's text or schema differs by platform, its entry has one tab per platform; the choice persists across pages.

On Windows and Linux, bare `cua-driver mcp` owns its runtime and stops it on stdin EOF. On macOS it proxies to the installed `CuaDriver.app` daemon so Accessibility and Screen Recording grants keep the app identity; `--socket` selects an explicit daemon endpoint on every platform. See [the process model](/cua-driver/concepts/how-cua-driver-works#processes-and-sessions) for the lifecycle.

## Common parameters

These parameters have the same shape on macOS, Windows and Linux (composed from shared schema fragments and held together by a CI consistency gate).

| Parameter | Tools | Meaning |
| --- | --- | --- |
| `session` | every action and cursor tool | Optional public run label. Repeat it on every call of multi-call work; it is not sticky. Without it, the call uses the connection's private implicit session. A label is never caller identity or authorization. |
| `target` | `move_cursor`, `click`, `drag`, `scroll`, `type_text`, `press_key`, `hotkey` | The preferred per-call target: `{kind:"window", pid, window_id}` or `{kind:"desktop", display_id:"primary"}`. Cannot be combined with the legacy `scope`, `pid` or `window_id`. |
| `delivery_mode` | `click`, `double_click`, `right_click`, `drag`, `scroll`, `type_text`, `press_key`, `hotkey` | `"background"` (the default) acts without fronting or raising the target. `"foreground"` briefly fronts it, acts, and restores the previous front window: use it only after a background attempt reports it did not land. Omitted or unknown values fall back to `"background"`. |
| `include_screenshot` | `get_window_state` | Default `true` (tree and screenshot). `false` returns the tree only, for re-indexing before an element action. |
| `capture_mode` | `get_window_state` | Deprecated and ignored; still accepted so older callers do not fail. |
| `modifier`, `button`, `element_token` | pointer and element tools | Held modifier keys, the mouse button, and the element handle from the latest `get_window_state` row. Action tools refuse `element_index` and `snapshot_id`. |

The `required` sets are the same on every platform: `click` requires nothing, `scroll` requires `direction`, and `zoom` requires `window_id` plus `x1`, `y1`, `x2`, `y2`. Legacy flat calls still check `pid`: a window action needs it, a desktop action omits it.

The first stateful call on a connection creates its implicit lifecycle session; later unnamed calls on that connection share it. The session's idle TTL is five minutes. Closing the connection, `end_session` or the idle timeout run the same cleanup.

Each `get_window_state` read replaces the previous snapshot of that window and lists the replaced ids in `invalidated_snapshot_ids`; a token from a replaced snapshot is refused with `stale_element_token`, which names the current snapshots. Window-relative `x, y` and `zoom` need a screenshot read of that window in the same session first (`screenshot_context_missing` otherwise), so pass one `session` label to the read and the actions of a one-shot `cua-driver call` sequence.

A window action may omit `window_id` only when its `pid` owns exactly one eligible top-level window. With several, the driver sends no input and returns `code: "ambiguous_window_target"`, `effect: "refused"` and the candidates; retry with an exact `window_id` from that result or from `list_windows`. A `pid` with no eligible window returns `window_target_not_found`.

## Action results

`click`, `double_click`, `right_click`, `drag`, `scroll`, `type_text`, `press_key`, `hotkey` and `set_value` return these structured fields:

| Field | Type | Meaning |
| --- | --- | --- |
| `path` | string | The delivery rung that ran: `ax`, `cgevent`, `cgevent_fg`, `key_events`, `key_events_fg`, `pixel`, `x11_atspi`, `x11_pixel`, `x11_pixel_fg` or `msaa`. |
| `verified` | boolean or absent | `true`: the effect was read back through accessibility. `false`: the action ran but is unconfirmed. Absent: the tool does not verify. |
| `effect` | string | `confirmed`, `unverifiable` or `suspected_noop`. |
| `escalation` | object or absent | Present when the driver recommends the next rung: `{recommended: "px" \| "foreground" \| "page", reason}`. |

`get_window_state` may return `degraded: true` with a `degraded_reason` when the accessibility walk found no actionable elements (bridge not up, not on the session bus, or a non-accessible surface). Treat `elements: []` as incomplete then, and act by pixel on the screenshot in the same response.
