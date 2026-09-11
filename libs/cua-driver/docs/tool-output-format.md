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

On Windows, `get_window_state.elements_complete` is true only when the
unprojected UI Automation walk visited every exposed node without a traversal
bound or enumeration failure skipping a node or subtree. Reaching
`max_elements` or `max_depth` exactly at the end of a tree remains complete.
`query` projects the returned Markdown and structured element rows, but does
not make the underlying snapshot incomplete; compare `total_element_count`
with `returned_element_count` to measure that projection.
