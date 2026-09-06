# Native runtime cancellation proof preparation

`production_cancel_proof.py` prepares a bounded cancellation test through two
independent `cua-driver mcp --direct` processes. Its local tests use synthetic
telemetry and mocked processes. **Native cancellation remains unproven.**

The test starts one drag, identifies its compositor lane from admission and
held-button telemetry, then starts the sibling drag. It sends `SIGKILL` to the
first drag's exact owned Driver process only after a fresh trace prefix shows
both buttons held and at least 100 ms of overlap. The last trace read must take
at most 250 ms, neither call may have returned, and both owned processes must
still be alive. A completed drag, stale read, or missing telemetry fails the
test instead of authorizing termination. There is still a scheduling race
between observation and termination; final trace validation rejects a drag
that completed in that interval.

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
- Each agent's `drag`: exactly `from_x`, `from_y`, `to_x`, `to_y`, and integer
  `duration_ms`. Choose distinct endpoints inside that app's fresh screenshot
  and a safe gesture on disposable content. Duration must be 1000–2000 ms;
  2000 ms gives the sibling's snapshot and overlap gate more time. The harness
  supplies the target, session, and background delivery mode.

The runner checks Driver's PID/window listing and a fresh image before each
drag, rejects changed bounds or endpoints outside the image, and records fresh
after-snapshots through its independent observer. It cannot establish that
coordinates have the intended application meaning; plan review must establish
that from the captured image. Slow snapshots can consume the first drag's
entire duration. That is a failed setup, not permission to replay it.

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
is replayed, including on failure.

Evidence includes the reviewed plan, provenance, MCP images/results,
`termination-prefix.json`, the stopped `trace.json`, observer after-snapshots,
and `result.json` with the exact terminated PID, signal, and monotonic request
and reap times. `cleanup.json` retains failures. Cleanup attempts every owned
Driver process, pending worker, trace connection, and primary-grab child even
when another cleanup fails; surviving owned children are killed and reaped.
Pre-existing app and foreground fixture processes are not terminated. Raw
evidence and local paths are for internal review.

Fresh-process lane reacquisition is omitted and always reported as
`reacquisition:"unproven"`. Saved-document effects, moving-primary cancellation,
target/keymap/display/session faults, repetition controls, and the complete
desktop matrix remain separate gates. A native run must establish both the
claimed cancellation behavior and exact loaded-artifact provenance before this
preparation can be described as certification.

Focused deterministic verification from the tests directory:

```text
python3 -m unittest production_cancel_proof_test production_realapp_proof_test realapp_proof_test primary_trace_test
```
