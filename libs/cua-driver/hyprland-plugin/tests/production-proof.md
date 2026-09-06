# Production input proof preparation

`production_realapp_proof.py` prepares a repeatable v3 native proof through
`cua-driver mcp --direct`. Each agent owns a separate Driver process. No test
signer, operator grant, input socket mutation command, or extra approval UI is
used. Existing signed runners and their historical results remain unchanged.

This harness has unit coverage. It has **not** certified a native candidate.
The pinned qualification targets are native LibreOffice Calc package
`26.2.5-3` and Inkscape package `1.4.4-6`; package versions alone are not proof
that either app received isolated input.

## Bounded real-app smoke

`production_app_smoke.py` is an earlier, narrower gate. Run it in a disposable
desktop with the exact source-built production plugin already loaded and
enabled. Pass `--source`, `--source-sha`, `--driver`, `--plugin`, and a new
`--evidence` directory. The runner verifies a clean source checkout, canonical
ALPM-owned app executables, and the plugin's current mapped path, device, and
inode in the active compositor.

It opens two new synthetic documents through direct Driver MCP, grounds each
keyboard action in fresh snapshots, and independently checks the saved files:
Calc A1 contains `a`; Inkscape's single rectangle moves two SVG pixels right.
The runtime uses unrestricted mode with explicit bypass acknowledgement.
Missing semantic grounding returns `inspection_only` and a nonzero exit code;
partial or unknown delivery fails without replay. App windows and evidence are
retained for inspection until the disposable environment is deleted.

This smoke does not prove foreground isolation, concurrency, pointer operations,
the policy matrix, or package installation. Its `synthetic_events` response is
a shared route family, not independent attribution to the loaded plugin.
`plugin_transport_attribution:false` keeps that limitation explicit. Use the
traced plans below for transport attribution and isolation. Mocked tests are
not native results.

## Reviewed native plans

Run only inside the prepared disposable desktop, after mapping one window per
app and the independent foreground journal fixture. Ground the exact window
identities, bounds, and gesture coordinates using fresh Driver snapshots.
The runner requires a clean exact-SHA checkout and verifies the active
compositor's mapped plugin by path, kernel device, and inode. A private
non-executable reference mapping of the exact candidate supplies the comparable
kernel identity, including on Btrfs where `stat()` can report a different
subvolume device. Replaced, changed, and deleted candidate files still refuse.
Qualified app processes
must resolve to canonical ALPM-owned executables with the GTK3 backend loaded;
an executable basename or recorded hash alone is insufficient. The cancellation
runner shares these checks. Store evidence outside the source checkout.
Review a JSON plan before execution. The plan has:

- `purpose`: `apps`, `policy`, `policy_cache`, `negative_control`, or `capacity`.
- `foreground`: its exact `pid` and `window_id`.
- `package_versions`: `{"libreoffice-fresh":"26.2.5-3","inkscape":"1.4.4-6"}`.
- `agents`: one or two objects (exactly three for capacity, one for policy_cache) with `app`, `target`, `bounds`, `name`, and
  `profile`. App proof requires distinct Calc and Inkscape processes. Public
  names may be identical; they are not runtime ownership credentials.
- `profile`: `mode` is `standard`, `bounded`, or `unrestricted`. Unrestricted
  requires `acknowledge_unrestricted:true`. A reviewed `manifest` path requires
  `approve_manifest:true` in every mode; bounded requires a manifest. These map
  to the normal direct-runtime environment contract. Managed/user policy remains
  inherited. Do not put secrets in the plan or evidence.
  The independent observer uses `unrestricted` with explicit bypass acknowledgement;
  the agent under test retains its reviewed profile and manifest ceiling.
- `phases`: sequential `{agent,tool,arguments}` objects, or `parallel` arrays
  with at most one call per runtime. Tools are click, press_key, hotkey, scroll,
  and drag. Reserved ownership/delivery arguments cannot override the plan.
- Optional per-step `smoke_stage` reuses the bounded keyboard smoke's semantic
  grounding. Calc accepts `insert` (`press_key`, `{"key":"a"}`), `commit`
  (`press_key`, `{"key":"Return"}`), and `save` (`hotkey`, `{"keys":["ctrl","s"]}`).
  Inkscape accepts `select` (`hotkey`, `{"keys":["ctrl","a"]}`), `move`
  (`press_key`, `{"key":"Right"}`), and `save` (`hotkey`, `{"keys":["ctrl","s"]}`).
  These steps use fresh default-depth snapshots and verify the exact sole app
  window before and after input. Missing semantic grounding or an unexpected
  dialog stops the run without recovery keys or replay. Unmarked reviewed
  steps retain their existing grounding contract. Saved-file and trace oracles
  are still required; a grounded shortcut alone does not prove its effect.
- Optional per-step `pointer_stage` derives coordinates from the same fresh
  full snapshot used before dispatch. Supply empty `arguments` and the matching
  tool: Calc `click_b2`/`click_a1` use `click`, `select_range` uses `drag`;
  Inkscape `click_rectangle` uses `click`, `move_rectangle` uses `drag`. Both
  apps accept `scroll_down`/`scroll_up` with `scroll`. These are app-effect
  cases only, not refusal or partial-delivery cases. They cannot also use
  `smoke_stage`. The native guest needs Python GI and GdkPixbuf for PNG reading.
  Calc requires the visible, regular cell grid and a unique vertical scrollbar;
  click/range cases require the top of the sheet and a changed name-field
  selection. Inkscape requires the unique blue synthetic rectangle. Its click
  case must start unselected and end selected; scroll must move the rendered
  canvas without changing document geometry; drag must move the rectangle in
  pixels and document coordinates without resizing it. The helper rejects
  clipped, ambiguous, missing, or scaled observations. Exact derived arguments
  and their source image are saved before the action. Both drags last 1.5 seconds
  with 30 steps; a parallel phase still needs `require_overlap:true` and a
  complete trace to prove actual overlap. Saved-output oracles remain required.
  This is proof preparation, not evidence that these native cells have passed.
  Drag proof separates the wire endpoint from app translation. Instrumented
  synthetic pointer enter/motion records append surface-local X/Y to the seven
  existing trace fields; primary cursor coordinates remain separate. The
  complete trace must show the requested start/end, an ordered straight path,
  balanced buttons, and uninterrupted drag focus. Inkscape's
  [1.4.4 selection tool](https://gitlab.com/inkscape/inkscape/-/blob/INKSCAPE_1_4_4/src/ui/tools/select-tool.cpp)
  anchors translation at its first processed motion, not button press. Its
  completed selection, unchanged dimensions, and pixel/document translation
  must agree, and the translation must match endpoint minus an actually
  delivered motion position. A missing endpoint, unmatched translation, or
  historical trace without synthetic coordinates fails this gate.
- Optional per-step `expect`: `{kind:"refused",reason:"<exact observed contract reason>"}`,
  `{kind:"partial"}`, or `{kind:"unknown"}`. The default is dispatched. Denial
  phases run serially so the no-dispatch interval contains no other action.
- For interrupted drags, `delivery.delivered_count:1` accounts for the
  acknowledged start phase, not a total event count. Explicit cancellation is
  partial background delivery; loss of the final reply is unknown delivery,
  retaining the start count when available. Neither means the document was
  rolled back or that later events could not have landed. Preserve the output
  and inspect fresh state without replaying the mutation.
- `outputs`: independent saved-document oracles with `agent`, `path`, `xpath`,
  optional `namespaces`, and exact `attributes` or `text`. SVG rectangle movement
  can use `rect_translation:[[min_x,max_x],[min_y,max_y]]`, which also rejects
  resizing. ODS files use `zip_member:"content.xml"`. App proof requires at least
  one output oracle per app. Save the documents through reviewed Driver actions.
- `require_overlap:true` requires at least 100 ms of traced drag overlap.
  `primary_point` optionally changes the foreground hold point, default `[300,300]`.
- `moving_primary:true` optionally moves the independent foreground primary grab
  during the reviewed Driver actions. The default remains parked. This mode
  requires `--trace-socket` and cannot be combined with `negative_control`.
  The helper repeats the historical 160 px square in 20 px steps, issuing a
  command every 100 ms after the preceding acknowledgement. The entire path
  must fit inside the foreground window and desktop; adjust `primary_point`
  using grounded geometry before running. At least one movement command and
  its acknowledgement must fall inside a Driver action's call interval.

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

For a capacity run, prepare a separate `purpose:"capacity"` plan with three
independent native app processes. Agents 0 and 1 must be Calc and Inkscape;
agent 2 can be another instance of either qualified app. Each process must own
exactly one mapped native window. A second window in the same app process does
not qualify. Ground each exact positive integer `pid` and `window_id`, bounds,
and safe action coordinates from fresh snapshots. The runner cross-checks the
PID/window pair against Driver's window listing before every snapshot. Use the
normal reviewed permission profiles described above; capacity adds no grant or
permission bypass.

Use exactly three serial phases: one successful action by agent 0, one by agent
1, then one action by agent 2 with
`expect:{"kind":"refused","reason":"lane_busy"}`. Omit `expect` for the first
two actions or set it to `{"kind":"dispatched"}`. Choose each action from a
fresh snapshot and retain its after-snapshot. Do not copy coordinates from an
unrelated app or evidence run. Capacity rejects parallel phases, extra phases,
moving-primary mode, and `require_overlap:true`. Persistent lane reservations
make simultaneous gestures unnecessary for this capacity check; use the
separate app plan to prove overlap and saved document changes.

Run the capacity plan with the command above and `--trace-socket`. A missing
trace option fails before any Driver process starts. The runner keeps all three
independent direct MCP processes alive through the third response. Each of the
first two intervals must contain v3 admission, actual input for the requested
tool, and completion in order on one compositor lane; together they must
exercise lanes 1 and 2. Runtime PIDs and session names alone cannot satisfy this
check. The third response must be an exact `lane_busy` refusal with no delivery
and no synthetic event in its serial trace interval. Another policy denial,
partial delivery, unknown effect, empty trace, missing instrumentation, or
reused lane fails the run without replaying the action.

Capacity evidence includes `capacity-agent-0-trace.json` through
`capacity-agent-2-trace.json`, containing the active trace prefixes around each
checked action, plus the normal stopped trace, continuous isolation, and
cleanup results. `result.json` records the observed lane for each admission and
the capacity verdict. Only an overall `result:"passed"` includes successful
isolation and cleanup checks. This proves bounded lane capacity and refusal,
not saved app effects, overlapping gestures, or the desktop matrix. Run the
capacity plan separately from the package smoke: it cannot pass without
instrumentation.

For a cached-connection tool-ceiling run, prepare a separate
`purpose:"policy_cache"` plan. Use exactly one qualified Calc or Inkscape process,
one fixed positive integer PID/window pair, and one nonempty session name.
The runner checks that pair against `list_windows` before each fresh snapshot.
Use an approved capability manifest with any of the three shared profiles;
`unrestricted` also needs `acknowledge_unrestricted:true`. The manifest must
allow session startup, window listing, window snapshots, and the permitted
input tool. Include the grounded resource scope required by those tools.
For `bounded`, include the required `expires_after` and `idle_timeout` fields.
Keep the profile, manifest, process, target, and session fixed for the entire
run; no policy reload, target override, extra signer, or per-window consent
mechanism is part of this proof.

Use exactly three serial phases, all with `agent:0`: a permitted input action,
an action using a different denied tool, then a fresh permitted action using
the first tool. For example, allow `click` and deny `press_key`, then review two
safe clicks and an Escape key call against fresh native state. The third call
is a new reviewed action, not a retry of either preceding call. Its arguments
can differ from the first call. Both permitted phases must omit `expect` or
use `{"kind":"dispatched"}`. Set the middle expectation to:

```json
{
  "kind": "refused",
  "reason": "permission_denied",
  "message": "Permission denied: capability manifest denies tool 'press_key'"
}
```

If the denied tool is omitted from the manifest instead of explicitly denied,
the only alternate message is
`Permission denied: tool 'press_key' is outside the capability manifest`.
Replace `press_key` with the actual middle-phase tool in either message. These
strings come from `authorize_tool_call_with_context` in
`rust/crates/cua-driver-core/src/authorization.rs`, the `AuthorizationError`
display in `policy.rs`. Direct MCP's early tool-admission check in `server.rs`
returns `isError:true`, `structuredContent:{"code":"permission_denied"}`, and
one MCP text content block with the exact reviewed message. Later common
admission in `tool.rs` uses `status:"refused"`,
`refusal.code:"permission_denied"`, and the exact reviewed `refusal.message`.
The verifier recognizes these two source-defined shapes, retains the raw
structured and text content, and requires no delivery in either case.
A generic plugin `permission_denied`, resource-scope denial,
managed/user-policy denial, partial result, or unknown delivery fails this
narrow manifest tool-ceiling case.

Run with `--trace-socket`; its absence fails before process launch.
`policy_cache` rejects parallel phases, moving-primary mode, and overlap claims.
The same direct MCP runtime must remain alive through all three responses.
Both permitted intervals must show ordered admission, tool input, and completion
on the same compositor lane. Runtime PID or session equality alone is
insufficient. The middle interval must contain zero synthetic-lane events,
including `agent_admitted`, the successful v3 `TARGET` admission marker emitted
by `input_experiment.cpp`. This trace measures that marker and input; it is not
a raw socket packet capture. The quiet interval includes the denied call's
before/after snapshots, and the runner also checks every gap between phases.
No action is replayed after a refusal, partial result, unknown delivery, or
transport failure.

Evidence includes `policy-cache-phase-0-trace.json` through
`policy-cache-phase-2-trace.json`, with continuous active prefixes, the stopped
trace, action runtime/session identities, exact refusal, foreground journal
checks, and cleanup results. `result.json.policy_cache` records the shared lane
and tool-ceiling verdict. Accept only an overall `result:"passed"`, which also
requires continuous foreground isolation and synthetic cleanup. This is
preparation for one cached-connection manifest tool-ceiling proof, not native
certification, the complete resource/managed-policy matrix, saved app effects,
or the desktop matrix. The focused tests use mocked runtimes and telemetry.

Moving-primary proof uses `primary-grab` in its independent `controlled` mode,
with `MOVE` commands and exact `MOVED` acknowledgements. Driver actions remain
background calls with before/after window snapshots. The foreground journal
must retain its click, key, scroll, and held-button state, and foreground window
identity and workspace must remain unchanged. Cursor endpoints may move along
the commanded path. The continuous trace must match every acknowledged position
in order, with no extra motion, focus changes, or foreground input events.
Every other traced position, including the final endpoint, must agree with the
latest cursor event; an unexplained position change leaves isolation unproven.
Missing, malformed, incomplete, or reordered command logs fail the run, as do
missing acknowledgements, an unjoined movement worker, or movement confined to
setup. The worker stops before trace collection and primary-button release.
Startup and movement acknowledgements both have bounded reads. If the worker
fails to stop, cleanup retains the command log, reaps the helper, and retries
the worker join; the run still fails.

Run the production package separately without the trace option. Successful
saved-output and foreground endpoint checks then produce a
`production-package-smoke` result with continuous isolation and synthetic
cleanup explicitly unproven. It cannot satisfy overlap or no-dispatch proof.
A moving-primary plan fails before process launch when the trace option is absent.
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
Moving runs also retain `primary-motion-commands.json` (ordered commands,
acknowledgements, and monotonic timestamps) and, once validated,
`expected-primary-motion.json`. The result identifies the primary mode and the
number of commands completed during Driver calls. Helper provenance includes
the independent `primary-grab` binary hash.

The runner closes and reaps every owned Driver child, escalates termination
only for those children, releases the foreground hold, and retains app files
after failures or partial/unknown delivery. Transport failures poison the
connection and are never replayed. Any cleanup failure fails the run.

Remaining native work includes executing the cached-connection plan at the exact
candidate SHA with verified loaded artifacts, successful plugin input on both
sides of the exact manifest denial, the quiet trace interval, continuous
foreground isolation, and cleanup. The complete mode/manifest/resource and
managed-policy allow/deny matrix remains separate. Other native gates include
execution of the moving-primary and capacity plans, runtime cancellation during overlapping gestures,
target/keymap/display/session faults, exact loaded-artifact
provenance, repeat controls, and the canonical desktop matrix. No existing
failed row is superseded by these helper tests.

Focused local verification:

```text
cd libs/cua-driver/hyprland-plugin/tests
python3 -m unittest production_realapp_proof_test realapp_proof_test primary_trace_test
```
