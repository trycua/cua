# Observe, act, verify

## Choose the route

Name the user's postcondition and any method constraint before acting.

1. For a non-GUI outcome, use a caller-provided application API, filesystem operation, or CLI when permitted; independently read back its result. An MCP-only client must not assume it has a shell.
2. For exact window geometry, prefer `set_window_frame` and verify with `list_windows`. For a known native menu path, use `invoke_menu` and verify the command's effect. It can temporarily activate the target, so apply the foreground authorization boundary.
3. For supported browser page content, use [BROWSER.md](BROWSER.md) when page-aware automation is permitted. GUI-only or native-input proof stays on the native route.
4. For native controls, prefer snapshot-bound accessibility actions. If semantics cannot reach the target, use a fresh, valid screenshot and the window pixel route.
5. If the selected background route refuses or visibly fails, consider explicitly authorized foreground delivery for that action.
6. If exact-window observation or delivery is unavailable, use the desktop route only when the task and authorization admit full-display capture and visible input.

Skip a route whose prerequisite is demonstrably absent: no valid window image means no window-pixel attempt. Do not exhaust impossible actions just to reach a later rung. A refusal never grants broader authority.

## Select the target

Discover running applications and their windows before assuming a remembered PID still exists. Use `launch_app` when the user requests or implies launch. Select the intended window from its returned `windows` or `list_windows({pid})`; never blindly take the first item.

A cold launch may return a process before a window appears. Retry bounded window discovery, not repeated launches. Empty discovery does not prove the process exited. For closure, absence from `list_windows` proves no listed windows, not process termination.

Choose the target on each action:

```json
{ "target": { "kind": "window", "pid": 844, "window_id": 10725 } }
```

```json
{ "target": { "kind": "desktop", "display_id": "primary" } }
```

These objects are argument fragments for tools advertising `target`, not standalone commands. Replace example IDs and coordinates with observed values. Do not combine `target` with flat `scope`, `pid`, or `window_id` input fields. Observation tools and semantic-only tools such as `get_window_state` and `set_value` retain their own `pid`/`window_id` schema.

`list_windows.z_index` uses larger integers for windows nearer the front. A null value is unknown, not zero; array order is not stacking evidence. Titles, bounds, and application identity help select among returned candidates but never replace an exact identifier.

## Observe

`get_window_state({pid, window_id})` requests the accessibility tree and a grounding screenshot by default. Check what actually came back: permission, backing-store, or surface-identity failures can leave usable tree data without an image. `screenshot_error` and `screenshot_frame_valid:false` are not empty-tree signals.

Prefer `structuredContent.elements` in MCP (the CLI prints structured fields directly) over parsing `tree_markdown`. Rows may contain `element_token`, role, label, value, actions, parent, depth, enabled/selected state, and frame. Missing fields are unknown.

- Use `query` to project matching rows plus ancestors without renumbering their indices.
- Use `max_elements` / `max_depth` to bound the walk, and compare returned/total counts. Truncation does not prove absence.
- Use `include_screenshot:false` only when tree-only observation is enough; it cannot ground a pixel action.
- Where advertised, `include_accessibility_tree:false` requests capture without a tree walk. Check the installed schema first.
- `capture_mode` is deprecated and ignored. Do not change configuration to repair a sparse tree.
- Use `screenshot_out_file` to save a PNG instead of inlining it, then actually read the image. Use an absolute, run-scoped output path.

The accessibility model may lag or disagree with rendered state: Electron text shims can echo writes, Catalyst may expose placeholders, and virtualized rows may have unusable frames. Cross-check the relevant visible outcome. An empty tree can also mean an unavailable accessibility bridge; inspect `degraded_reason` and the platform guide. Retry once for lazy initialization, not indefinitely.

## Act once

Use an opaque `element_token` from the latest snapshot of the intended window. If using an integer, pair `element_index` with that response's `snapshot_id`. Do not derive or edit tokens. A later snapshot can invalidate a pending action, including when another agent observes the same window.

Example CLI window action, with IDs and token replaced from the preceding response:

```bash
cua-driver click '{"target":{"kind":"window","pid":844,"window_id":10725},"element_token":"s0000002a:14","session":"run-1"}'
```

| Intent                          | Arguments in addition to exact target/session                                  |
| ------------------------------- | ------------------------------------------------------------------------------ |
| Activate a control              | `click` with `element_token`                                                   |
| Double/right click              | `double_click` / `right_click`; inspect the host schema and delivery result    |
| Insert text                     | `type_text` with `element_token` and `text`                                    |
| Focus a visible field and type  | `type_text` with `x,y,text`, when that form is advertised                      |
| Replace an exposed native value | `set_value` with `pid,element_token,value` (its own schema, no `target`)       |
| Send a key or combination       | `press_key` with `key`, or `hotkey` with `keys`                                |
| Scroll / drag                   | Inspect `describe scroll` / `describe drag`; units and supported shapes matter |

Text insertion and value replacement are different intents. Setting a field does not prove a form submission, navigation, or rename committed. Open a collapsed search/input control and reobserve before typing into it; one focus-click may not both open and focus it. Inspect the existing value/selection before replacing content.

If a text action returns `unverifiable`, take a fresh snapshot before retrying. A deferred provider can publish after the call unwinds, so retrying immediately may duplicate text. If renderer focus is missing, the advertised `type_text` pixel form focuses then types in one call. For minimized windows, prefer an exposed semantic commit control; do not assume Return or a value write commits, and do not silently restore the window.

Keep `delivery_mode:"background"` as the default for window input. The route may use accessibility hit-testing even when addressed by pixels: pixel coordinates do not promise physical pointer delivery. Read the returned `route` instead of inferring it from the tool name.

## Pixel coordinates

Ground window actions on the PNG from that exact `get_window_state`; ground desktop actions on `get_desktop_state`. Origins are top-left, increasing downward. The driver handles its own window capture scaling; do not add window offsets to window-local input.

The harness may downsample the displayed preview independently of the returned PNG. Use the returned dimensions and the original file. If measuring on a resized preview, account for its exact scale in both axes; do not assume the preview is native resolution. Do not guess from accessibility frames or another app's geometry.

After movement, resize, navigation, or a competing desktop interaction, reobserve. When using `zoom`, read its schema and preserve the `from_zoom` mapping on the supported follow-up action. A manually cropped image requires its crop offset; an untracked crop is not an action coordinate source.

For tiny targets, inspect at full resolution or annotate a copy without changing its dimensions. Keep the raw evidence unmodified. `debug_image_out`, where advertised, captures an action diagnostic; it is not a pre-action approval step.

## Desktop loop

Use the full-display path when authorized and the narrower route cannot satisfy the task. It is global input to the visible desktop, not background targeting. Ensure the desired app/field is visible and focused before keyboard input; stop if the user or another controller changes it.

```bash
cua-driver get_desktop_state '{"session":"run-1","screenshot_out_file":"/absolute/run-dir/before.png"}'
# Read before.png; replace x/y with the observed target.
cua-driver click '{"target":{"kind":"desktop","display_id":"primary"},"x":600,"y":300,"session":"run-1"}'
cua-driver get_desktop_state '{"session":"run-1","screenshot_out_file":"/absolute/run-dir/after.png"}'
# Read after.png and verify the intended outcome.
```

Desktop calls use the portable `display_id:"primary"` target. Do not assume arbitrary display IDs work. A desktop action does not change the modality or authority of later calls; the next action can target a window again.

## Verify and stop

For an expressible exact-window postcondition, use `verify_state` with bounded predicates and stable samples. Its result is `satisfied`, `unsatisfied`, or `unknown`. The driver does not interpret its optional final image; the agent does. Read `unknown_reason`: ambiguous matches, unavailable observations, untrusted web state, or insufficient stable samples are not success.

```bash
cua-driver verify_state '{"pid":844,"window_id":10725,"expect":[{"element":{"selector":{"label_contains":"Saved"},"exists":true}}],"include_screenshot":true,"session":"run-1"}'
```

This example proves a matching trusted element exists, not that every application has a meaningful “Saved” indicator. Choose predicates that establish this task. Use fresh `get_window_state` for outcomes the predicate language cannot express, and fresh `get_desktop_state` for desktop proof. `verify_state` remains an exact-window tool; a previous desktop action does not disable it.

Action facts are not task outcomes:

| Field                | Meaning for the next decision                                                                         |
| -------------------- | ----------------------------------------------------------------------------------------------------- |
| `effect:"confirmed"` | The action has publishable readback; still check the task postcondition                               |
| `partial`            | Only the reported delivered portion landed; inspect before repair                                     |
| `unverifiable`       | Delivery cannot prove effect; observe before retrying                                                 |
| `suspected_noop`     | Available evidence suggests no useful change                                                          |
| `refused`            | The route deliberately did not deliver                                                                |
| `route`              | Public actuator class: `accessibility`, `synthetic_events`, `global_input`, `dom`, or `trusted_input` |
| `escalation`         | Suggested next route/reason, never authorization or an automatic retry                                |

Successful action escalation uses `target` (`pixel`, `foreground`, or `page`) and `reason` (`route_unavailable`, `delivery_failed`, `effect_unconfirmed`, `suspected_noop`, or `permission_required`). Error diagnostics may have a different shape; inspect their code and current schema. Missing action facts are not confirmation.

When the postcondition is visibly satisfied, stop acting. For media, selection alone is not playback: check the title, playing/pause indicator, and elapsed progress after preroll/buffering. For closure, verify the requested window disappeared. State only what the evidence proves.

Never replay canceled, partial, or unknown actions automatically. An interrupted transport may have delivered input before losing its response. Report the interruption accurately and obtain fresh state before any further action.

## Filesystem outcomes and GUI fallbacks

When the requested outcome is a filesystem change and the caller has a
headless filesystem or command capability, use that semantic route. Enumerate the
exact source set, decide the destination-conflict policy before changing
anything, perform one batch-safe operation, then independently read back both
source and destination manifests. Do not open a file manager merely to mimic a
move, copy, or rename that the caller can execute and verify directly.

If the caller has no such capability, use the file manager as a GUI fallback
and keep each claim narrow:

1. After entering an inline rename and setting its value, commit it with the
   platform's confirmation key, then take a fresh snapshot. Value readback from
   the inline editor proves only that the editor changed; it does not prove the
   filesystem rename committed.
2. For a multi-selection, use the platform modifier (`cmd` on macOS, `ctrl` on
   Windows/Linux). With authorization for visible control on macOS and Windows, issue that modified click with
   `delivery_mode:"foreground"` so the target observes physical modifier state;
   a refused background attempt is an escalation signal, not a failed action to
   trust or repeat. Re-snapshot before the next operation. Continue only when
   every intended item is selected and the prior selection was preserved.
3. After a cross-window drag or paste, verify the destination contains the
   complete expected set and the source reflects copy-versus-move semantics.
   A delivered drag, keypress, or menu action is not file-operation proof.
4. If a destination conflict presents an unrecognized policy or ambiguous
   partial result, stop that GUI path and surface the unresolved state instead
   of retrying blindly.

### Clipboard outcomes and GUI fallbacks

When the requested postcondition is an exact value on the system clipboard,
rather than the literal gesture of selecting and copying it, keep the operation
semantic. Read the value from the narrowest typed source, call
`clipboard_write`, then prove the real clipboard state with `clipboard_read`.
For browser content, this means reading the page with `get_browser_state` and
writing the exact observed text; a passive page-text ref does not need to be
clicked first.

Use visual selection followed by the platform copy hotkey only when the user
explicitly asks for that gesture, the source cannot expose the value
semantically, or direct clipboard tools are unavailable. Treat that as a GUI
fallback: re-snapshot before acting, verify the selected range when the
application exposes it, and escalate only the delivery step that cannot land
in the background.
