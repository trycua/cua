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

## Explicit lifecycle

Use the signed catalog distributed with the reviewed candidate and inspect the
exact target before changing local state:

```bash
cua-driver extension inspect cua-perception --catalog <catalog.json>
cua-driver extension install cua-perception --catalog <catalog.json>
cua-driver extension status cua-perception
```

Inspection must identify the extension version, platform target, protocol
range, capabilities, artifact sizes and hashes, publisher, licenses, notices,
and provenance. Do not substitute an unsigned archive or infer component
availability from the repository-wide latest release.

Inspect a replacement before running `extension update`. Remove only the
extension-owned installation with `extension remove cua-perception`. Neither
operation changes the default Driver or an external `cua-som` installation.

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
[perception third-party notices](perception-third-party-notices.md). A release
must include the reviewed notices, ledgers, SBOM, source/conversion materials,
and any corresponding-source offer required for the way the artifact is
distributed or offered over a network.
