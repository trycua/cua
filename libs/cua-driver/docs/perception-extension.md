# Optional perception extension

`parse_visual_regions` is a model-neutral Cua Driver tool backed by the
separately installed `cua-perception` extension. It parses one screenshot into
text and icon regions. It does not capture the screen, choose an action, or
send input.

The extension is absent from the default MIT-licensed Driver installation.
Driver startup, capture, parsing, and update checks never install it or
download model weights. A missing extension returns `not_installed`, allowing
the caller to continue with accessibility, typed browser state, or its own
visual reasoning.

## Local image CLI

The read-only CLI convenience mode parses an existing local PNG through the
same installed worker and canonical visual-region validation:

```bash
cua-driver perception parse \
  --image /tmp/window.png \
  --capture /tmp/capture.json \
  --json
```

The capture metadata file has this strict shape:

```json
{
  "source": { "kind": "window", "pid": 844, "window_id": 10725 },
  "snapshot_id": "s0000002a",
  "captured_at": "2026-09-18T12:00:00Z"
}
```

For the primary desktop, use
`{"source":{"kind":"primary_desktop","display_id":"primary"}}`.
`snapshot_id` and `captured_at` are optional. The command validates and hashes
the PNG header and declared image bounds, but it does not decode pixels in the
privileged CLI, create or retain a Driver capture, or include the local input
paths in its output. The JSON result has schema `cua.visual_regions_v1` plus a
`local_input` provenance block that sets `action_eligible` to `false` and
`action_authority` to `none`. Its `local_png_<digest>` label uses a separate
namespace from Driver `capture_<namespace>_<sequence>` IDs and cannot authorize
a Driver action.

Because `--json` is required, every failure is written to stdout as one stable
envelope and the process exits nonzero:

```json
{
  "ok": false,
  "error": {
    "code": "invalid_capture_metadata",
    "message": "captured_at must be an RFC3339 timestamp",
    "retryable": false
  }
}
```

CLI admission codes are `invalid_arguments`, `input_open_failed`,
`input_not_regular_file`, `input_too_large`, `input_read_failed`,
`invalid_capture_metadata`, `invalid_png`, and `internal_error`. Worker and
extension failures preserve the visual parsing codes, including
`not_installed`, `artifact_invalid`, `incompatible_protocol`, timeout, worker,
resource-limit, and inference failures. An optional `detail` string provides
diagnostic context and must not be parsed as a stable field.

## Explicit lifecycle

Each Cua Perception release is published as the `cua-perception-v<version>`
GitHub release. It carries one signed catalog and one archive per target:

| Target | Catalog | Archive |
| --- | --- | --- |
| macOS arm64 | `cua-perception-<version>-aarch64-apple-darwin.catalog.json` | `cua-perception-<version>-aarch64-apple-darwin.tar.gz` |
| Linux x64 | `cua-perception-<version>-x86_64-unknown-linux-gnu.catalog.json` | `cua-perception-<version>-x86_64-unknown-linux-gnu.tar.gz` |
| Windows x64 | `cua-perception-<version>-x86_64-pc-windows-msvc.catalog.json` | `cua-perception-<version>-x86_64-pc-windows-msvc.tar.gz` |

The release also carries each target's SBOM, redacted provenance, runtime
contract, the release gate's install evidence, and a `SHA256SUMS` file. The
Driver resolves the archive named in the catalog relative to the catalog file,
so download both into the same directory. Select the release by its
`cua-perception-v*` tag; it is never marked as the repository's Latest release.
Inspect the exact target before changing local state:

```bash
cua-driver extension inspect cua-perception --catalog <catalog.json>
cua-driver extension install cua-perception --catalog <catalog.json>
cua-driver extension status cua-perception
```

Inspection must identify the extension version, platform target, protocol
range, capabilities, artifact sizes and hashes, publisher, licenses, notices,
and provenance. Do not substitute an unsigned archive or infer component
availability from the repository-wide latest release.

Each catalog expires one year after its release commit. After that, install
from a newer `cua-perception-v*` release.

## Release publication

Cua Perception follows Release Please with no manual publish step. Merging its
release pull request bumps `VERSION`, and the release workflow anchors the
merged commit with a lightweight `cua-perception-v<version>` tag. That tag push
runs `.github/workflows/cd-cua-perception.yml`, which:

1. requires the tag to be a lightweight tag on `main` that matches `VERSION`;
2. verifies the reviewed OmniParser ONNX conversion against
   `scripts/artifacts.lock.json`;
3. builds the worker on each target, assembles the bundle, and runs its health
   check, self-test, and real parse;
4. re-verifies, packages, and signs each target in the reviewed candidate
   workflow, whose signing job runs in the `cua-perception-candidate-signing`
   environment;
5. installs the published Cua Driver with its canonical installer on each
   target and runs `extension inspect`, `install`, `status --self-test`,
   `perception parse`, and `remove` against the signed catalog; and
6. publishes the verified assets only when every job passed for the exact tag
   commit.

Pull requests that change this pipeline run steps 1 through 3 plus an unsigned
package and a developer-only install check. They never sign or publish.

Inspect a replacement before running `extension update`. Remove only the
extension-owned installation with `extension remove cua-perception`. Neither
operation changes the default Driver or an external `cua-som` installation.

Install, update, and remove take effect on a running Driver without a restart.
A running daemon or `cua-driver mcp` session checks the installed extension
before each `parse_visual_regions` call and fully reverifies it whenever the
installation changed. A parse that is already running finishes with the version
it started with.

## Capture-bound parse and action

Keep capture, parse, action, and reobservation on one persistent MCP connection
or typed SDK runtime. Each one-shot CLI process owns a disposable capture
registry, so a later process cannot resolve its `capture_id`.

1. Call `get_window_state` for the exact native `(pid, window_id)` target. Use
   `get_desktop_state` only when full-desktop observation is explicitly allowed
   and a window capture cannot represent the task.
2. Retain the returned native `capture_id`, source, PNG dimensions, and
   action-coordinate mapping. An accessibility `snapshot_id` is a different
   identity.
3. Call `parse_visual_regions` with that capture ID and bounded `kinds`,
   `min_confidence`, and `max_regions` options.
4. Prefer a current accessibility element or typed browser ref. If pixels are
   required, choose a point inside one current region and send one `click` with
   `x`, `y`, the exact target, `delivery_mode`, and the same `capture_id`.
5. Reobserve after every action attempt. Never remove `capture_id` and retry an
   expired, stale, retired, or mismatched capture as an unbound click.

Capture-bound click refusals use one shared code table on every platform,
owned by `CaptureActionError::wire_code` in `cua-driver-core`:
`capture_id_invalid`, `capture_not_found`, `capture_expired`,
`capture_generation_mismatch`, `capture_target_mismatch`,
`capture_coordinate_invalid`, and `capture_frame_mismatch`. Live-target
inspection failures before admission remain adapter-owned
(`capture_action_refused` on Windows and Linux, `capture_target_mismatch` on
macOS).

A capture-bound click consumes the capture before native dispatch. At most one
action may derive from a capture. Capture again after an action, timeout,
unknown result, resize, move, scroll, navigation, display-layout change, or
target-identity change.

Window parsing does not imply background input support. Driver first applies
the selected platform's normal background route. A structured
`background_unavailable` refusal does not authorize a foreground retry.
`delivery_mode:"foreground"` may change focus, workspace, or the system cursor
and requires the workflow's existing foreground authorization. Desktop input
is foreground/system input even when the parsed screenshot came from Driver.

## Worker boundary and errors

The worker receives only the admitted PNG and bounded parse options. It has no
desktop capture, accessibility, browser, input, credential-store, or provider
credential authority. The local worker performs no network access and does not
require Python.

Treat `capture_not_found`, `capture_expired`, `capture_stale`, and
`capture_generation_mismatch` as a request to capture again. Preserve
`unsupported_target` and `unsupported_platform` as explicit limitations. Stop
using the extension after `incompatible_protocol` or `artifact_invalid` until
the exact installed artifact is inspected. Never act from a partial result
after a worker failure, timeout, invalid frame, or resource-limit error.

## Distribution boundary

The extension artifact has its own license and source obligations. The current
candidate ledgers identify the OmniParser detector artifact as AGPL-3.0-only,
the PP-OCR detector and recognizer artifacts as Apache-2.0, and the packaged
ONNX Runtime by an exact version and hash selected at assembly time. Read the
[perception third-party notices](perception-third-party-notices.md), including
its precautions for redistribution and hosted services. A release
must include the reviewed notices, ledgers, SBOM, source/conversion materials,
and any corresponding-source offer required for the way the artifact is
distributed or offered over a network.
