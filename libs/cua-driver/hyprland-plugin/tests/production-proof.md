# Production input proof preparation

`production_realapp_proof.py` prepares a repeatable v3 native proof through
`cua-driver mcp --direct`. Each agent owns a separate Driver process. No test
signer, operator grant, input socket mutation command, or extra approval UI is
used. Existing signed runners and their historical results remain unchanged.

This harness has unit coverage. It has **not** certified a native candidate.
The pinned qualification targets are native LibreOffice Calc package
`26.2.5-3` and Inkscape package `1.4.4-6`; package versions alone are not proof
that either app received isolated input.

Run only inside the prepared disposable desktop, after mapping one window per
app and the independent foreground journal fixture. Ground the exact window
identities, bounds, and gesture coordinates using fresh Driver snapshots.
Review a JSON plan before execution. The plan has:

- `purpose`: `apps`, `policy`, or `negative_control`.
- `foreground`: its exact `pid` and `window_id`.
- `package_versions`: `{"libreoffice-fresh":"26.2.5-3","inkscape":"1.4.4-6"}`.
- `agents`: one or two objects with `app`, `target`, `bounds`, `name`, and
  `profile`. App proof requires distinct Calc and Inkscape processes. Public
  names may be identical; they are not runtime ownership credentials.
- `profile`: `mode` is `standard`, `bounded`, or `unrestricted`. Unrestricted
  requires `acknowledge_unrestricted:true`. A reviewed `manifest` path requires
  `approve_manifest:true` in every mode; bounded requires a manifest. These map
  to the normal direct-runtime environment contract. Managed/user policy remains
  inherited. Do not put secrets in the plan or evidence.
- `phases`: sequential `{agent,tool,arguments}` objects, or `parallel` arrays
  with at most one call per runtime. Tools are click, press_key, hotkey, scroll,
  and drag. Reserved ownership/delivery arguments cannot override the plan.
- Optional per-step `expect`: `{kind:"refused",reason:"<exact observed contract reason>"}`,
  `{kind:"partial"}`, or `{kind:"unknown"}`. The default is dispatched. Denial
  phases run serially so the no-dispatch interval contains no other action.
- `outputs`: independent saved-document oracles with `agent`, `path`, `xpath`,
  optional `namespaces`, and exact `attributes` or `text`. SVG rectangle movement
  can use `rect_translation:[[min_x,max_x],[min_y,max_y]]`, which also rejects
  resizing. ODS files use `zip_member:"content.xml"`. App proof requires at least
  one output oracle per app. Save the documents through reviewed Driver actions.
- `require_overlap:true` requires at least 100 ms of traced drag overlap.
  `primary_point` optionally changes the foreground hold point, default `[300,300]`.

Invoke from the exact source checkout, substituting the already reviewed paths
and candidate SHA:

```text
python3 libs/cua-driver/hyprland-plugin/tests/production_realapp_proof.py \
  --source <checkout> --source-sha <candidate-sha> \
  --driver <driver> --plugin <loaded-plugin-file> \
  --primary-grab <primary-grab> --foreground-journal <fixture-journal> \
  --plan <reviewed-plan.json> --evidence <new-evidence-directory> --record-video
```

For instrumented certification, build v3 with `CUA_HYPRLAND_INPUT_TRACE=ON`
and add `--trace-socket <runtime>/cua-input-v3.sock`. The runner reuses
`primary_trace.py` and the independent foreground hold. Trace hooks must be
complete, ordered, and free of overflow/timeouts. A refusal must contain the
exact reason, refused effect, no delivery, and no synthetic event in its trace
interval. A policy error alone never proves no dispatch. Trace instrumentation
is disabled in production builds.

Run the production package separately without the trace option. Successful
saved-output and foreground endpoint checks then produce a
`production-package-smoke` result with continuous isolation and synthetic
cleanup explicitly unproven. It cannot satisfy overlap or no-dispatch proof.
A `negative_control` plan contains only `{negative_control:true}` phases and
requires the trace; it passes only when the independent warp-and-return is
detected despite identical cursor endpoints. It is not an isolation pass.

Evidence retains the reviewed plan, source SHA/branch/dirty status, source and
Driver versions, helper/binary hashes, package and native process identities,
loaded-plugin listing, MCP images/results, timeline, saved files before/after,
cleanup failures, and available trace/video. The loaded-plugin listing and
file digest must still be reconciled with installation/build provenance by the
native operator; this helper cannot independently establish a loaded module's
build origin. Local paths and raw evidence are for internal review.

The runner closes and reaps every owned Driver child, escalates termination
only for those children, releases the foreground hold, and retains app files
after failures or partial/unknown delivery. Transport failures poison the
connection and are never replayed. Any cleanup failure fails the run.

Remaining native work includes the complete mode/manifest allow/deny matrix,
moving-primary proof, runtime cancellation during overlapping gestures,
third-lane refusal, target/keymap/display/session faults, exact loaded-artifact
provenance, repeat controls, and the canonical desktop matrix. No existing
failed row is superseded by these helper tests.

Focused local verification:

```text
cd libs/cua-driver/hyprland-plugin/tests
python3 -m unittest production_realapp_proof_test realapp_proof_test primary_trace_test
```
