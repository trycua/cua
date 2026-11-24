# Cua Driver MCP tool output format

Every call keeps the standard MCP `ToolResult` envelope:

- `content` contains human-readable text and optional images;
- `structuredContent` contains the machine-readable successful result;
- `isError` distinguishes tool failure from a successful outcome.

Text is diagnostic, not a stable parsing API.

## Action tools

Successful pointer, keyboard, value, and browser-input actions return the
closed shared `ActionResult` in `structuredContent`:

```json
{
  "effect": "unverifiable",
  "route": "global_input",
  "delivery": {"mode": "foreground"},
  "escalation": {"target": "page", "reason": "effect_unconfirmed"}
}
```

Do not parse platform route names, coordinates, targets, or the removed
`verified` bit from text. Use `effect`, `route`, `delivery`, `evidence`, and
`escalation`, then call `verify_state` or take a fresh snapshot for the task
postcondition. See [Action results and postcondition
verification](action-result-contract.md).

## Observation and state tools

Observation tools retain their typed tool-specific structured payloads.
`get_window_state`, for example, returns the accessibility outline and element
records in `structuredContent` and can attach a PNG as image content. A
multimodal harness interprets the image; Cua Driver does not OCR it or assign
task meaning.

The `get_window_state` text header repeats the published `snapshot_id`
(`window_id=… pid=… snapshot_id=s…`), so a model whose client shows it only
text content can form `element_token: "<snapshot_id>:N"` from a row `[N]` in
the tree. This includes `full_output:true` and empty trees when a snapshot was
published; observation-only reads without a snapshot omit the field. Programs should
still read `snapshot_id` and `element_token` from `structuredContent`.

On Windows, `get_window_state.elements_complete` is true only when the
unprojected UI Automation walk visited every exposed node without a traversal
bound, timeout, or enumeration failure skipping a node or subtree. Reaching
`max_elements` or `max_depth` exactly at the end of a tree remains complete.
`query` projects the returned Markdown and structured element rows, but does
not make the underlying snapshot incomplete; compare `total_element_count`
with `returned_element_count` to measure that projection. Partial MSAA trees,
unusable UIA trees, and process-root fallbacks remain incomplete because they
cannot prove the exact target window's full UIA tree.
Internal `verify_state` reads also remain incomplete while the Windows
structured projection omits display-only UIA nodes. A complete native walk
does not make that projection exhaustive.

On macOS, `get_window_state.elements_complete` is true only when the requested
window resolved and the unprojected accessibility walk gave nothing up: no
`max_elements` or timeout stop (`truncated: true` names that one), no node
past `max_depth`, no node whose `AXChildren` read failed, and no closed menu
left collapsed. A plain read collapses the menu bar's closed menus, so it
reports `false` for any window with a menu bar; a `query` or
`full_output: true` read walks the menus. As on Windows, `query` projects the
returned rows without changing `elements_complete`.

`elements` holds addressable rows only. With `tree_format: "elements"` or
`"both"`, `get_window_state` also returns `observations`: the display-only
rows of the same walk (static text, labels, read-only values) with `role`,
`depth`, `label`, `value`, `frame` and `parent_index`, but no `element_index`
or `element_token`. No action tool accepts them. A `query` keeps the
observations whose role, label or value contains it. The list stops at 500
rows and then carries `observations_truncated: true`. A platform that cannot
observe these rows leaves `observations` out, which means not observed, never
empty. `verify_state` searches `elements` first and `observations` after, so
a predicate on display text such as Calculator's result can be satisfied.
It treats a missing element as absent only when the walk is complete and
`observations` is present and not truncated; otherwise the result is
`unknown`. Values are compared exactly, including bidi marks such as U+200E.
