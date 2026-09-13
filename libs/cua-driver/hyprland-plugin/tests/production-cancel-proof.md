# Native runtime cancellation proof preparation

`production_cancel_proof.py` prepares a bounded cancellation test through two
independent `cua-driver mcp --direct` processes. Its local tests use synthetic
telemetry and mocked processes. An optional recovery phase starts a new direct
runtime after cancellation. **Native cancellation and recovery remain unproven.**

The test grounds both gestures before starting either drag. It starts one
drag, identifies its compositor lane from admission and
held-button telemetry, then starts the sibling drag. It sends the preselected
termination signal to the first drag's exact owned Driver process only after a fresh trace prefix shows
both buttons held and at least 100 ms of overlap. The last trace read must take
at most 250 ms, neither call may have returned, and both owned processes must
still be alive. A completed drag, stale read, or missing telemetry fails the
test instead of authorizing termination. There is still a scheduling race
between observation and termination; final trace validation rejects a drag
that completed in that interval.

Set `termination_signal` to `SIGKILL` (the default) or `SIGTERM` in the
reviewed plan. The SIGTERM case fails if the process does not exit within three
seconds; it does not substitute SIGKILL to pass. Unconditional cleanup can
still terminate a surviving owned child after that failure. These are process
termination tests, not evidence for MCP request cancellation or `end_session`.

Only ordinary Driver calls target the apps. The trace connection uses `HELLO`,
`TRACE_START`, `TRACE_READ`, and `TRACE_STOP`; it never sends target, input,
grant, or cancellation protocol commands. An independent `primary-grab`
fixture holds the foreground button at a parked position. This fixture is the
primary-seat oracle, not an actuator for either app. Every Driver runtime,
including the observer, uses the existing unrestricted permission mode with
explicit approval bypass through `production_mcp.py`. This harness creates no
signer, grant, approval panel, or permission profile.

## Prepare the native gate

Use the prepared disposable Hyprland desktop and the exact source candidate.
Reconcile the loaded plugin with the build artifact before interpreting any
result. The existing production provenance helper checks the source SHA,
records versions, hashes, dirty status, package identities, and loaded plugins.
It requires native Calc `libreoffice-fresh 26.2.5-3` and Inkscape `1.4.4-6`, each
in a separate process with exactly one mapped native window. The foreground
journal fixture must be a third process. Preserve the application documents
for inspection after interruption.

Create a reviewed JSON plan from fresh Driver window listings and snapshots.
The plan fields are:

- `purpose`: `"cancellation"`.
- `kill_agent`: `0` or `1`. This agent starts first and is the sole deliberate
  termination target; the other agent is the sibling.
- `foreground`: exact positive integer `pid` and `window_id` values.
- `primary_point`: two integer coordinates relative to the foreground window,
  safely inside its bounds and the desktop.
- `package_versions`: `{"libreoffice-fresh":"26.2.5-3","inkscape":"1.4.4-6"}`.
- `agents`: exactly two objects with distinct `app` values `calc` and
  `inkscape`, a `name`, exact `target` (`pid`, `window_id`), and reviewed
  `bounds` (`x`, `y`, `width`, `height`). An optional `profile` must equal
  `{"mode":"unrestricted","acknowledge_unrestricted":true}`.
- Prefer `pointer_stage:"select_range"` for Calc and
  `pointer_stage:"move_rectangle"` for Inkscape, each with `drag:{}`. The
  runner derives a 1500 ms, 30-step gesture from the exact fresh full snapshot's
  pixels and semantic state. These stages require the synthetic documents
  described in `production-proof.md`; ambiguous, clipped, scaled, or already
  satisfied state refuses before dispatch.
- For a separately reviewed coordinate plan, each agent's `drag` contains
  exactly `from_x`, `from_y`, `to_x`, `to_y`, and integer
  `duration_ms`. Choose distinct endpoints inside that app's fresh screenshot
  and a safe gesture on disposable content. Duration must be 1000–2000 ms. The harness
  supplies the target, session, and background delivery mode.

The runner checks Driver's PID/window listing and a fresh image for each drag,
then retains both observations and derived arguments before dispatch. It
positively resolves Calc's current name-field selection and requires the next
Standard toolbar to establish that the Formula Tool Bar section is complete.
A missing or truncated selection field is unknown, not an unselected range.
It
rejects changed bounds, endpoints outside the image, and grounding older than
five seconds at dispatch. Before either drag, both observations must retain
250 ms for dispatch scheduling. If not, the runner can refresh both observations
once, retaining each attempt separately. A second stale pair fails without
input. A final paired check includes evidence-write and primary-guard time.
This reserve is not a worst-case scheduling guarantee: each actual dispatch
still checks its own five-second limit. No snapshot refresh or input retry
runs after either input attempt begins.
The independent observer records fresh after-snapshots for both apps. When
using pointer stages, the sibling must also show the expected selection or
rectangle movement. The victim's interrupted effect remains uncertain and is
preserved without replay. Explicit coordinate plans still require human
review of their application meaning. Slow setup fails; it never authorizes a
retry of an uncertain action.

Run only after the native environment and plan are authorized and prepared.
Substitute the reviewed paths and exact candidate SHA:

```text
python3 libs/cua-driver/hyprland-plugin/tests/production_cancel_proof.py \
  --source <checkout> --source-sha <candidate-sha> \
  --driver <driver> --plugin <loaded-trace-plugin-file> \
  --primary-grab <primary-grab> --foreground-journal <fixture-journal> \
  --trace-socket <runtime>/cua-input-v3.sock \
  --plan <reviewed-plan.json> --evidence <new-evidence-directory>
```

Build the production v3 plugin with `CUA_HYPRLAND_INPUT_TRACE=ON` for this gate.
An uninstrumented package smoke cannot substitute for cancellation evidence.

## Optional fresh-runtime recovery

Add `recovery:{"pointer_stage":"click_b2"}` when Calc is the victim, or
`recovery:{"pointer_stage":"scroll_visible"}` when Inkscape is the victim.
Calc also supports `click_a1`. Both agents must use the derived pointer stages
above so the sibling's app effect is verified. Choose a Calc cell that will
not already be selected after the interruption; already-satisfied or ambiguous
state fails without another action. Inkscape recovery requires the interrupted
rectangle to remain visible and uniquely selected. Before any recovery input,
`scroll_visible` chooses up or down from the same fresh image, toward the larger
visible canvas margin. It requires 100 pixels of margin for the pinned fixture's
approximately 80-pixel scroll. The evidence records the requested and resolved
stages. Pixel-size, document-geometry, and scroll-effect checks remain strict;
clipping after dispatch fails without replay. The explicit `scroll_down` stage
remains available for separately reviewed plans.

The runner first verifies the cancellation phase, including victim release and
normal sibling completion. It saves the victim's fresh observer snapshot and
image hashes in `interrupted-state.json` before launching recovery. These are
evidence of observed interrupted state, not saved-document or rollback proof.
The runner does not issue a document save or infer which on-disk file holds
unsaved changes.

Recovery creates a third action runtime through `mcp --direct`, records its
distinct process ID, and uses a new session name. The killed runtime remains
poisoned and reaped. The new process loads the unrestricted startup profile
and inherited managed/user policy through the existing normal runtime path;
the evidence records that startup configuration, not an independent inspection
of every effective policy rule. The new call still requires common dispatch
admission and fresh compositor admission.

The new runtime checks the app's canonical executable/GTK identity, exact
PID/window listing, reviewed bounds, and a full fresh image. It derives one
new click or scroll from that observation. Grounding older than five seconds
refuses before dispatch. This action has a different tool and meaning from
the canceled drag; no canceled arguments, snapshot tokens, or transport are
reused. Target replacement, moved geometry, denied policy, unreadable state,
or failed effect verification fails the recovery phase without replay.

The independent observer then captures the result, and the existing app
oracle must prove the new selection or viewport change. Lost replies remain
unknown, with their observer snapshot retained when observation succeeds.
Continuous telemetry must show exactly one fresh admission and one completed
click or scroll on the released victim lane, with no sibling input, replayed
drag, cancellation, held synthetic input, or primary-seat disturbance. Both
victim indexes and both compositor lane mappings are supported.

`cancellation-prefix.json` and `recovery-prefix.json` retain complete active
trace prefixes. Phase analysis adds an in-memory stop sentinel only; it never
resets or stops the real trace between phases. Final cleanup still requires
the real stopped trace, unchanged prefix history, balanced synthetic input,
and uninterrupted primary isolation. After recovery completion, synthetic
events may only cancel an idle lease or leave its pointer/keyboard resources;
any new admission, input, or gesture marker fails cleanup. A recovery or cleanup
failure fails the overall run even when the cancellation phase was verified.

## Interpret evidence

An overall `result:"passed"` requires one cancellation on the victim lane,
one subsequent release of its held synthetic button, and normal sibling
release and drag completion after that cancellation. The sibling must return
a dispatched response and remain alive. Both lanes must finish with balanced
synthetic input. Complete continuous telemetry must show no primary cursor,
focus, keyboard, scroll, or button disturbance. Foreground identity, workspace,
cursor, and journal counters must also remain unchanged while the grab is held.

The victim's lost response remains `outcome:"unknown"`; it does not mean no
input was delivered or that the document was rolled back. If a response arrives,
the helper retains it and accepts only the existing partial/unknown delivery
contract. A successful victim response fails this cancellation test. The killed
connection is poisoned before termination and is never reused. Neither action
is replayed, including on failure. Optional recovery is a separately grounded
new action, not a retry.

Evidence includes the reviewed plan, provenance, MCP images/results,
`agent-N-drag-grounding.json` with each exact image and derived arguments,
`termination-prefix.json`, the stopped `trace.json`, observer after-snapshots,
and `result.json` with the exact terminated PID, signal, and monotonic request
and reap times. `cleanup.json` retains failures. Cleanup attempts every owned
Driver process, pending worker, trace connection, and primary-grab child even
when another cleanup fails; surviving owned children are killed and reaped.
Pre-existing app and foreground fixture processes are not terminated. Raw
evidence and local paths are for internal review.

Recovery runs additionally retain `recovery-grounding.json` (startup profile,
runtime identity, app identity, snapshot, arguments, and oracle),
`recovery-action.json`, and `recovery-after.json`. The interrupted snapshot and
image hashes are checked again after successful recovery. The new runtime is
included in unconditional cleanup, including failed setup and lost replies.

Without `recovery`, the cancellation-only contract is unchanged and reports
`reacquisition:"unproven"`. With it, `reacquisition.result:"verified"` requires
the new app effect and complete recovery trace. Saved-document effects remain
explicitly unproven in either mode. Moving-primary cancellation,
target/keymap/display/session faults, repetition controls, and the complete
desktop matrix remain separate gates. A native run must establish both the
claimed cancellation behavior and exact loaded-artifact provenance before this
preparation can be described as certification.

Focused deterministic verification from the tests directory:

```text
python3 -m unittest production_cancel_proof_test production_pointer_grounding_test production_realapp_proof_test realapp_proof_test primary_trace_test
```
