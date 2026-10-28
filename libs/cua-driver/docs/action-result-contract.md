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
  "evidence": [{"kind": "value_readback", "detail": "value read back as 42"}],
  "summary": "Set value to 42 on spin button 'Width' (delivery_mode=background, path=x11_atspi)"
}
```

`effect` and `route` are required. `summary` repeats the result's text
content (resolved `window_point` / `screen_point`, the element hit, popups
that opened and the call that targets them, focus outcome, retargeting) so a
client that hands the model only `structuredContent` still sees everything a
raw MCP client reads from `content`. `evidence[].detail` is the readback the
evidence rests on. A `refused` result adds `error: {code, hint}`.

| Field | Values |
| --- | --- |
| `effect` | `confirmed`, `partial`, `unverifiable`, `suspected_noop`, `refused` |
| `route` | `accessibility`, `synthetic_events`, `global_input`, `dom`, `trusted_input` |
| `delivery.mode` | `background`, `foreground`, `not_applicable`, `unknown` |
| `evidence[].kind` | `value_readback`, `window_change` |
| `evidence[].detail` | optional human readback behind the evidence |
| `summary` | optional; the result text, verbatim |
| `error.code` / `error.hint` | only with `effect: refused` |
| `hint` | optional; one short sentence saying what to do next (see [Recovery hints](#recovery-hints)) |
| `next_call` | optional; `{tool, arguments}`, a call that is safe to run as is (see [Recovery hints](#recovery-hints)) |
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

The contract is deliberately closed. It does not echo selectors, request
coordinates, scope, targets, platform transport names, diagnostic pointers, or
the old `verified` boolean as fields; `summary` is prose for the agent, not a
machine-readable echo of the request.

The invariants are:

- `confirmed` has publishable readback or window-change evidence;
- `partial` has `delivery.delivered_count`;
- `refused` has neither delivery nor evidence;
- `error` appears only on `refused`.

## Recovery hints

A refusal, or an action that ran but was not confirmed, tells the agent what
to try next. Two optional top-level fields carry it, on every tool and
platform:

- `hint`: one short sentence (at most 240 characters) saying what to do next.
- `next_call`: `{"tool": "...", "arguments": {...}}`, the exact call that moves
  the task forward. It is always safe to run as is: a read such as
  `get_window_state`, `list_windows`, `get_browser_state` or
  `check_permissions`, or a call that keeps the user's window where it is. A
  foreground retry, or anything that needs the user's consent, is never a
  `next_call`. The `hint` names it with its condition, for example
  `delivery_mode:"foreground"` "only if the user allows the window to come
  forward".

They appear on three kinds of result:

| Result | Where the fields sit |
| --- | --- |
| An error result (`isError: true`) with a `code` | next to `code` in `structuredContent` |
| A browser refusal (`status: "refused"`, `refusal.code`) | next to `refusal` in `structuredContent` |
| An `ActionResult` with `effect` `unverifiable`, `suspected_noop` or `refused` | `hint` and `next_call` on the `ActionResult` |

For example, a background drag that macOS cannot drop returns:

```json
{
  "code": "background_unavailable",
  "reason": "drag_and_drop_session",
  "hint": "No drop landed: macOS drag-and-drop needs the real pointer. Try a route that is not a drag first (Move button, menu item, shortcut). Retry with delivery_mode:\"foreground\" only if the user allows the window to come forward.",
  "next_call": {"tool": "get_window_state", "arguments": {"pid": 811, "window_id": 659}}
}
```

A producer that already sets `hint` or `next_call` keeps its own value. In
`run_steps`, a failed step's report carries the same `hint` and `next_call`; in
`run_script`, the hint is appended to the error message of the failed call.
In the typed SDK records, `next_call.arguments` is the same JSON object as
JSON text.

The codes with a hint are listed in the `RULES` table in
`rust/crates/cua-driver-core/src/recovery_hint.rs`; its tests cover each one.

### Batching suggestion

An agent that acts one call at a time pays a model turn per action. After three
direct action calls in a row in one session (reads do not count, and a pause of
over 60 seconds starts the count again), the third result also carries a `hint`
that suggests chaining the next steps with `run_steps`, and so does every third
after it. On an `unverifiable` result the suggestion follows the recovery hint
in the same string. It is advice only: it has no `next_call`, it never rides on
an error, and it is never sent for a step that already runs inside `run_steps`
or `run_script`. The logic is in `rust/crates/cua-driver-core/src/batch_nudge.rs`.

`suspected_noop` is advisory. It means a bounded readable state remained
unchanged, not that the action failed. For a ref-targeted browser DOM click,
the readback covers bounded semantic and form state on the target plus the
document URL. An unchanged DOM click readback remains `unverifiable`, with
readback evidence: effects elsewhere on the page can be invisible to that
readback, so verify the expected postcondition before retrying.

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
