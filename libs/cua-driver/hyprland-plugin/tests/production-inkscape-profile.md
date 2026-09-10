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
continuous trace isolation unproven. Separate fresh-session package lifecycle
and independent primary-input observations remain required for shipping.

`production_app_smoke.py --app-profile inkscape-only` runs the existing bounded
single-app keyboard smoke with only the Inkscape package gate and SVG fixture.
It uses the production artifact role and the same source/manifest flags. Its limits remain explicit: no
concurrency, complete isolation, or full desktop certification claim.
Fault/cancellation helper plans may also select `app_profile`; retain their
existing required scenario fields and supply `document` on native app agents.

Portable regression command (Python 3.10 or newer):

```sh
cd libs/cua-driver/hyprland-plugin/tests
python3 -m unittest discover -b -p 'production*_test.py'
```
