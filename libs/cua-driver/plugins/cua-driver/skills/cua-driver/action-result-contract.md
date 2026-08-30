# Action results and postcondition verification

Cua Driver 0.15 separates two facts that earlier releases mixed together:

- `ActionResult` says what route the driver used and how strongly it can
  account for the action itself.
- `VerifyStateOutput` says whether a caller-defined postcondition is
  `satisfied`, `unsatisfied`, or `unknown`.

The driver reports these facts. The agent harness owns task meaning, visual
reading, stop/retry decisions, and movement through the action ladder.

## MCP action result

Every successful action returns a closed `structuredContent` object:

```json
{
  "effect": "confirmed",
  "route": "accessibility",
  "delivery": {"mode": "background"},
  "evidence": [{"kind": "value_readback"}]
}
```

`effect` and `route` are required.

| Field | Values |
| --- | --- |
| `effect` | `confirmed`, `partial`, `unverifiable`, `suspected_noop`, `refused` |
| `route` | `accessibility`, `synthetic_events`, `global_input`, `dom`, `trusted_input` |
| `delivery.mode` | `background`, `foreground`, `not_applicable`, `unknown` |
| `evidence[].kind` | `value_readback`, `window_change` |
| `escalation.target` | `pixel`, `foreground`, `page`, `session` |
| `escalation.reason` | `route_unavailable`, `delivery_failed`, `effect_unconfirmed`, `suspected_noop`, `permission_required` |

The action-result tools are:

`click`, `double_click`, `right_click`, `scroll`, `drag`, `mouse_drag`,
`parallel_mouse_drag`, `move_cursor`, `mouse_button_down`, `mouse_button_up`,
`type_text`, `type_text_chars`, `press_key`, `hotkey`, `set_value`,
`set_window_frame`, `invoke_menu`, `browser_click`, `browser_pointer`, and
`browser_type`.

Other mutating tools such as application launch, window activation, browser
navigation, dialogs, uploads, and downloads retain their own typed results.

The contract is deliberately closed. It does not echo selectors, coordinates,
scope, targets, platform transport names, diagnostic pointers, or the old
`verified` boolean.

The invariants are:

- `confirmed` has publishable readback or window-change evidence;
- `partial` has `delivery.delivered_count`;
- `refused` has neither delivery nor evidence.

## Window target resolution

Window-scoped actions accept a PID without `window_id` only when that process
has exactly one eligible top-level window. The driver promotes that unique
match to an exact `(pid, window_id)` target before dispatch. If the PID owns
multiple eligible windows, the action fails before sending input with
`code: "ambiguous_window_target"`, `effect: "refused"`, and a `candidates`
array containing each candidate's `window_id`, title, application name when
available, and on-screen state. Call `list_windows({pid})`, select the intended
candidate, and retry with its explicit `window_id`.

A PID with no eligible windows fails with `code: "window_target_not_found"`.
Explicit `window_id` targets and `element_token` targets retain their exact
resolution semantics; the guard does not replace or reinterpret them.

An action that reached an actuator but lacks a trusted readback is
`unverifiable`, not `confirmed`. Screenshot change, native API acceptance,
event receipt, and operator observation may remain useful internal diagnostics,
but they do not independently justify `confirmed`.

## Verification remains separate

After an action, use `verify_state` for a bounded structured postcondition.
`satisfied` is the only successful terminal status. `unsatisfied` can justify a
retry or another ladder route. `unknown` means the available observation could
not prove either answer and must never be promoted to success.

When `include_screenshot` is enabled, the screenshot is uninterpreted evidence.
A multimodal harness reads it and decides whether to stop, retry, or advance.

## SDK access

Rust, Python, and TypeScript keep the transport-neutral `ToolResult` envelope:
text, images, structured JSON, error state/code, degraded state, and raw JSON.
The ambiguous `verified` field is removed.

Successful action calls expose the typed value at `result.action`; successful
`verify_state` calls expose it at `result.verification`. In Rust the equivalent
borrow accessors are `result.action()` and `result.verification()`.

```python
result = await driver.click(click_input)
if result.action.effect is ActionEffect.CONFIRMED:
    verification = await driver.verify_state(expectation)
    if verification.verification.status is VerificationStatus.SATISFIED:
        return "done"
```

```ts
const result = await driver.click(input)
if (result.action?.effect === ActionEffect.Confirmed) {
  const checked = await driver.verifyState(expectation)
  if (checked.verification?.status === VerificationStatus.Satisfied) {
    return "done"
  }
}
```

## Escalation belongs to the harness

An optional escalation is advice, not an automatic retry:

| Target | Harness action |
| --- | --- |
| `pixel` | refresh visual state and choose an exact pixel target |
| `foreground` | explicitly select foreground delivery when session policy permits |
| `page` | bind the native window to a supported browser page route |
| `session` | prepare or explicitly widen the session only when policy permits |

SDK integrators, OpenClaw, Hermes, and other agent hosts can implement different
policies above this same narrow fact contract without duplicating platform
actuator details.

## Migration from 0.14

- Replace `result.verified` checks with `result.action.effect` for action facts.
- Use `result.verification.status` only for `verify_state` postconditions.
- Replace imports of the removed `ClickOutput`, `DesktopActionOutput`, and
  `MoveCursorOutput` types with `ActionResult`.
- Do not read coordinates, `scope`, `path`, `transport`, or request targets from
  an action response; retain request context in the caller if it is needed.
- `move_cursor` no longer echoes `x`/`y`; call `get_cursor_position` when the
  observed pointer location is needed.
- Treat `unverifiable` as unknown action effect, not failure and not success.
- Treat MCP `isError` as transport/tool failure; inspect a successful
  `ActionResult` separately.
- Upgrade daemon and SDK together. A 0.15 SDK intentionally rejects legacy
  0.14 action payloads instead of guessing at their meaning.
