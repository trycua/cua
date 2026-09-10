# Bounded Inkscape-only qualification profile

This is a harness interface, not a native certification result. It does not
change production app admission, qualify current LibreOffice, or replace the
complete native Hyprland `scripts/ci/linux/run-rust-e2e.sh` all suite and its
required evidence. The default `calc-inkscape` profile remains unchanged.

Use `"app_profile": "inkscape-only"` in a reviewed
`production_realapp_proof.py` plan. Its exact package inventory is
`"package_versions": {"inkscape": "1.4.4-6"}`. Each agent must name
`"app": "inkscape"`, an absolute synthetic `document` SVG path, and its exact
`target: {pid, window_id}`. Keep the existing session, permission `profile`,
window `bounds`, foreground fixture, phases, and action-grounding fields.
The app qualification profile is separate from each Driver permission profile;
unrestricted sessions still require `acknowledge_unrestricted: true`.

Prepare two distinct native Inkscape processes for `purpose: "apps"`, and
three for `purpose: "capacity"`. This runner consumes prelaunched targets.
Verify the installed application's supported independent-process launch method;
do not assume a `--new-instance` flag. The single-app smoke already launches a
positional document and independently verifies that its process is new.
The profile checks the canonical executable `/usr/bin/inkscape`, exact ALPM
ownership/version, GTK3 mappings, one native Wayland client per PID, exact
Hyprland address, document title, and the absolute document argument in the
process command line. Shared PIDs, duplicate windows/documents, XWayland,
dialogs, stale targets, and unsupported grounding fail the proof.

Each app lane needs its own saved SVG oracle, bound to that agent's `document`:

```json
{
  "agent": 0,
  "path": "/synthetic/lane-0.svg",
  "format": "svg",
  "xpath": ".//svg:rect[@id='smoke-rectangle']",
  "namespaces": {"svg": "http://www.w3.org/2000/svg"},
  "rect_translation": [[2, 2], [0, 0]]
}
```

Set translation bounds to the reviewed action's expected effect. The harness
requires a changed native SVG, an identified rectangle with unchanged size,
and nonzero movement within finite bounds. Intermediate pointer episodes keep
the existing no-save contract; the save episode checks both lanes' outputs.
The existing exact keyboard `smoke_stage` and image-derived `pointer_stage`
paths remain available for each Inkscape lane.

Keep `require_overlap: true` on the drag episode and retain the traced overlap
threshold. Capacity remains a separate serial plan: agents 0 and 1 dispatch,
then agent 2 receives exactly `{"kind":"refused","reason":"lane_busy"}`.
Trace evidence must identify compositor lanes 1 and 2 and no third dispatch.
The new profile also checks both earlier reservations and their epochs across
the refusal while the original Driver runtimes remain alive. Existing primary
trace, negative control, fresh snapshots, no-retry, and input-cleanup checks
remain required. Capacity does not itself prove saved app effects or overlap.

## Source and artifact identities

Every production proof helper accepts the existing `--source` and
`--source-sha` for the original product checkout, plus the optional pair
`--harness-source` and `--harness-sha` for the checkout containing this runner.
Both checkouts must be clean Git roots at their exact full SHAs. Without the
pair, the runner must belong to the product checkout as before. Detached state
is recorded with an empty branch. Neither checkout identity alone proves how
a binary was built. Driver/module hashes and the active module's exact kernel
mapping checks remain recorded and enforced.

The new app profile requires an explicit artifact role:

- `--artifact-role diagnostic --trace-socket /path/to/trace.sock` identifies
  the instrumented proof. Do not pass production kit manifests for that module.
- `--artifact-role production --kit-manifest /path/to/KIT-PROVENANCE.json
  --profile-manifest /path/to/PROFILE.json
  --build-provenance /path/to/BUILD-PROVENANCE.json` identifies the trace-disabled
  package. Supply all three files together and omit `--trace-socket`.

Production checks bind the raw profile digest, kit source revision, embedded
build source, exact production CMake flags, and built module digest to the
actual mapped module. All three manifest digests are retained. Kit tooling
revision is recorded separately and can differ from the later harness revision.
This binding complements the packaging verifier and package integrity checks;
it is not a package signature, compiler/runtime compatibility check, or native
certification. Diagnostic attribution cannot certify trace-disabled bytes.
Production runs retain the existing `production-package-smoke` scope and leave
continuous trace isolation unproven. Explicit production `production_realapp_proof.py`
runs now require the independent primary observer below. Separate fresh-session
package lifecycle evidence remains required for shipping.

`production_app_smoke.py --app-profile inkscape-only` runs the existing bounded
single-app keyboard smoke with only the Inkscape package gate and SVG fixture.
It uses the production artifact role and the same source/manifest flags. Its limits remain explicit: no
concurrency, complete isolation, or full desktop certification claim.
Fault/cancellation helper plans may also select `app_profile`; retain their
existing required scenario fields and supply `document` on native app agents.

## Independent primary gate for trace-disabled bytes

`primary_observer_fixture.py` is a test-only replacement for the generic
foreground GTK fixture in this explicit gate. It needs native GTK3 PyGObject
and pycairo; no compilation is needed. It creates no synthetic input. Start it
from the clean harness checkout in the disposable native Wayland session,
using fresh paths (the journal, wire file, and control socket must not exist):

```sh
cd libs/cua-driver/hyprland-plugin/tests
observer_dir=$(mktemp -d)
python3 "$(pwd)/primary_observer_fixture.py" \
  --journal "$observer_dir/foreground.jsonl" \
  --wire "$observer_dir/foreground.wire" \
  --control "$observer_dir/control.sock" \
  --lifetime-ms 600000 >"$observer_dir/fixture.stdout" &
```

The fixture owns its `WAYLAND_DEBUG=client` capture, enforces the Wayland
backend, and maps `Cua Isolated Input Foreground`. Bind the plan's exact
foreground PID and native window ID to this process using fresh snapshots.
Use the existing independently built `primary_grab` helper to hold its left
button, as the real-app runner normally does. Add these arguments to the
production-role real-app proof with its existing source, package manifest,
app-plan, primary-grab, and evidence arguments:

```sh
--foreground-journal "$observer_dir/foreground.jsonl" \
--primary-observer "$observer_dir/control.sock"
```

This gate is explicitly parked-primary: `purpose: "apps"` and
`purpose: "negative_control"` are supported. The normal app plan still requires
two independent runtimes, two exact native app targets, and both saved SVG
oracles (except the existing intermediate pointer episodes). All Driver input
uses the ordinary fresh-snapshot route. Moving-primary, compositor lane
attribution, actual compositor overlap, and no-dispatch capacity remain the
existing diagnostic trace gates. A pair of overlapping tool calls is not
proof of overlapping compositor delivery; `require_overlap` cannot pass on
this no-trace path.

Before the first action and after all actions and agent-runtime cleanup, the
reader sends a fresh nonce over the fixture's local control socket. It verifies
the socket's kernel peer PID/UID, exact native foreground window, process
start identity, and fixture source digest. Each acknowledgement follows two
`Gdk.Display.sync()` roundtrips with a bounded GTK event drain. The reader also
requires their matching Wayland `sync`/`callback.done` wire records, so a
heartbeat or an old file alone cannot satisfy a boundary.

The retained interval must enclose every action's request/response interval.
It ends before the primary button is released. Log files must retain the same
device/inode and unchanged prefixes, complete newline-delimited records,
unbroken journal sequence and monotonic timestamps, matching producer identity,
and fresh state heartbeats with no gap over one second. The fixture and reader
enforce 32 MiB/100,000-record limits, two-second sync deadlines, and an action
interval of at most 60 seconds. A deadline, missing acknowledgement, truncated
record, changed file, stalled event loop, unknown wire record, or exceeded limit
fails qualification. Raw begin/end journal and wire evidence are retained.

Baseline wire evidence must show one primary pointer and keyboard on the same
surface, a held left button, and no held keyboard keys/modifiers. During the
parked interval, an exact same-position `wl_pointer.motion` notification on the
same primary object may pass only with unchanged focus, held input, and
foreground interaction. Retain and count these duplicates; they do not certify
the negative control. Any displacement, including an excursion and return,
foreign-pointer motion, enter/leave, button, axis, key, keyboard-focus,
seat-capability, or corresponding journal focus/grab/input transition fails.
GTK journal motion events and counter changes still fail because they do not
independently establish device identity and a baseline. See the
[duplicate-notification contract](production-proof.md#inkscape-only-environment-profile)
for the complete constraints. The foreground client counters and
held-button state must remain unchanged, and the independent compositor
cursor/focus/workspace endpoint checks must also match. This establishes
client-observed primary continuity under the implicit held-button grab; it
does not expose hidden compositor grab state or provide plugin transport
attribution. The existing `continuous_isolation` and `synthetic_cleanup`
trace claims remain `unproven` on this path. The separate
`independent_primary_isolation` result names its narrower scope.

Run a separate `purpose: "negative_control"` plan with
`"phases": [{"negative_control": true}]`, the same production artifact role,
and the same observer interface. Keep the primary point at least 40 pixels
from the right edge and 30 from the bottom of the fixture. The existing
`primary_grab ... canary` sends an excursion and return in one roundtrip.
The same normal detector must return `failed`, even when GTK coalesces motion
and final cursor positions match. Control acceptance additionally requires
wire motion away and back on the same primary pointer, with only motion-related
violations. A detector that reports `passed`, never observes the excursion,
or fails for another reason cannot pass the control. The enclosing successful
control run reports `production-package-primary-control`; its isolation result
deliberately remains `failed`.

These interfaces and portable tests record no new native certification.
Native normal/control runs must still be performed on the exact mapped
trace-disabled package. Use fresh sessions for diagnostic versus production
modules and preserve their separate artifact identities.

Portable regression command (Python 3.10 or newer):

```sh
cd libs/cua-driver/hyprland-plugin/tests
python3 -m unittest discover -b -p 'production*_test.py'
```
