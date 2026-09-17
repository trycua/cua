# Migrate to the typed native-window SDK

The next breaking Cua Driver release adds typed app discovery, window discovery,
and native-window snapshots, and changes the SDK `click` input and return value.
These examples target that release. Version 0.25 already supports native-window
operations through the generic tool surface; it does not provide this typed SDK
contract. Upgrade the generated bindings and native library together.

## Replace generic window calls

Use the generated methods and records directly:

| Operation | TypeScript | Python | Result |
| --- | --- | --- | --- |
| Discover running apps | `listApps(ListAppsInput.new({}))` | `list_apps(ListAppsInput())` | `ListAppsOutput.apps` |
| Discover an app's windows | `listWindows(ListWindowsInput.new({ pid }))` | `list_windows(ListWindowsInput(pid=pid, on_screen_only=None))` | `ListWindowsOutput.windows` |
| Observe an exact window | `getWindowState(GetWindowStateInput.new({ pid, windowId }))` | `get_window_state(GetWindowStateInput(...))` | `WindowStateOutput` |

Python's generated input constructors require explicit `None` for omitted
optional fields. The complete example linked below supplies every window-state
input field.

Select a unique app and window using identities your application knows. An app
may own several windows, and ordering is not a selection contract. Do not take
the first result or substitute the active window when the intended window is
missing. TypeScript window IDs are `bigint`; preserve them without conversion to
`number`.

`WindowStateOutput.elements` contains structured accessibility elements when
available; screenshot-only captures can omit it. Use the snapshot's element
tokens for actions, and inspect its optional snapshot ID, screenshot metadata,
and degradation fields before relying on a capture.
`elementsComplete` / `elements_complete` can be false for a healthy actionable
projection. A missing element does not prove absence, and a unique returned
label establishes uniqueness only among the returned elements. Use control
identities and postconditions that your application knows.
`images` carries base64 image data and MIME types directly. You no longer need
to parse `ToolResult.structuredJson` / `structured_json` for these three methods.
Other SDK methods retain their existing return types.

## Replace click coordinates and handle the direct result

`ClickInput` requires an exact `ActionTarget`, a `ClickPosition`, and an explicit
`InputDeliveryMode`. Replace the old top-level `x` and `y` fields with
`ClickPosition.Coordinates` in TypeScript or `ClickPosition.COORDINATES` in Python.
For an accessibility element, use `ClickPosition.Element` or
`ClickPosition.ELEMENT` with its snapshot token. Set `button`, `count`, and
`session` when needed.

`click` returns `ActionResult` directly. Remove `result.isError` / `is_error`,
`result.text`, and JSON-envelope parsing from click-only code. A refused click
raises `DriverError.Tool`; it is not a successful `ToolResult` carrying an error
flag. Mixed wrappers must distinguish `ActionResult` from the `ToolResult`
returned by methods such as `typeText` / `type_text` and `pressKey` / `press_key`.

An action result describes delivery, not proof that the UI reached the intended
state. Capture a fresh window snapshot and verify an application-specific
postcondition. After a timeout, inspect the UI before retrying: the action may
have landed even when the caller did not receive a result.

## Preserve targeting and delivery semantics

- Request background delivery explicitly. If the OS or application cannot
  deliver that action in the background, handle the refusal. Do not retry with
  foreground delivery automatically; that changes focus and needs a deliberate
  caller decision.
- Treat element tokens as snapshot-bound. If a token is stale, capture the same
  exact window again and resolve a unique element from the fresh snapshot.
  Never rewrite or reuse the stale token for another window or session.
- Check platform limitations and degradation metadata. macOS, Windows, X11,
  and Wayland do not offer identical accessibility, capture, or background
  input routes. An unavailable route is not permission to choose another target.
- Keep coordinate spaces distinct. Window screenshot coordinates belong to
  that exact window capture; desktop screenshot coordinates belong to the
  selected display. For a window target, use coordinates from its fresh window
  screenshot, not screen-absolute coordinates; the driver performs the window
  translation. Inspect screenshot dimensions, scale, and frame validity, and
  do not reuse coordinates after window geometry changes or manually rescale
  an image without accounting for that transformation.

See [Use Cua Driver in process](https://cua.ai/docs/how-to-guides/driver/use-sdk-in-process)
for complete Python and TypeScript discovery, token-click, verification, and
shutdown examples. The [agent SDK adapters](../examples/agent-sdks/) show how to
retain desktop coordinate tools while migrating the click result handling.
