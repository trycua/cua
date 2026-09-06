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

On Windows, `get_window_state` explicitly distinguishes the two coordinate
spaces in its structured payload:

- `elements[].frame` is in `screen_physical_px`, as declared by
  `element_frame_coordinate_space`. Frames wholly outside the exact target
  `window_bounds` are omitted and carry
  `frame_reliability:"outside_target_window"` instead.
- Pixel-action `x,y` values are in `window_screenshot_px`, as declared by
  `pixel_action_coordinate_space`. When a screenshot is present,
  `pixel_action_to_screen` gives its screen origin and physical-screen pixels
  per action pixel. Convert with
  `screen = screen_origin + action * screen_pixels_per_action_pixel`.

The accessibility elements remain one structured `elements` array; coordinate
metadata does not add a second rendering of the tree.
