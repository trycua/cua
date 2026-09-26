# Optional visual-region parsing

Use `parse_visual_regions` when a current Cua Driver screenshot contains useful
text or icons that accessibility and typed browser state do not expose. The
tool returns model-neutral observations. It does not decide that a region is
interactive, choose an action, or grant permission to act.

This capability is a developer preview delivered by the separately installed
`cua-perception` extension. It is absent from the default Cua Driver
installation. The default local Driver remains MIT licensed. The reviewed
candidate records the OmniParser detector artifact as AGPL-3.0-only and the
PP-OCR detector and recognizer as Apache-2.0. The packaged ONNX Runtime must
report its exact version, hash, license, and notices. Inspect the exact catalog,
manifest, SBOM, source ledger, model ledger, and built artifact rather than
applying the Driver license to them. Publication remains blocked while the
OmniParser ledger says `license-review-required`; distribution and remote
service require the applicable notices, corresponding source/conversion
material, and AGPL network source path.

## Inspect before installing

Never install the extension merely because a parse returned `not_installed`.
Use the catalog distributed with the reviewed release candidate, then inspect
the exact candidate:

```bash
cua-driver extension inspect cua-perception --catalog <catalog.json>
```

Review its version, target, capabilities, protocol range, worker and model
sizes, hashes, destination, licenses, publisher, and provenance. Installation
must be an explicit caller decision after that review:

```bash
cua-driver extension install cua-perception --catalog <catalog.json>
cua-driver extension status cua-perception
```

Use `cua-driver extension update cua-perception --catalog <catalog.json>` only
after inspecting the proposed replacement. Use
`cua-driver extension remove cua-perception` to remove extension-owned artifacts
when they are not in use.

Verified-catalog publication and the exact initial OCR/model artifact are still
pending release verification. If `inspect` cannot present authenticated
metadata for the exact artifact, stop. Do not invent an artifact URL, substitute
an unsigned archive, or describe the preview as a stable release.

Installation is never a side effect of Driver startup, capture, parsing, or
update checks. Parsing does not download code or model weights. The extension
is Rust-based and does not install Python. Driver does not request, store, or
forward Jev or any other decision-provider credential.

## Capture, parse, act once, and reobserve

Use one persistent MCP connection or one typed SDK runtime for the stateful
loop. A one-shot CLI tool call owns a disposable runtime, so a later process
cannot resolve its `capture_id`. Keep the four stages separate within the same
runtime:

1. Capture the exact native window with `get_window_state`, or an explicitly
   authorized desktop with `get_desktop_state`. Use a
   native-resolution capture when the preview requires it and retain the
   returned `capture_id`, target, dimensions, and action-coordinate mapping.
2. Call `parse_visual_regions` with that `capture_id` and bounded options. For
   example, request only `text`, set `min_confidence`, and cap `max_regions`.
3. Prefer accessibility or typed browser evidence when it identifies the same
   control. If a pixel action is necessary, construct one complete action
   locally from one region, validate that its bounds and target still match the
   source capture, then dispatch one `click` containing `x`, `y`, the exact
   target, `delivery_mode`, and that same `capture_id`. Never retry a capture
   refusal without `capture_id`. Dispatch at most one action derived from that
   capture.
4. Reobserve after every action attempt, including a timeout, unknown result,
   partial delivery, or suspected no-op. Verify the postcondition from fresh
   state before choosing another action.

Example parse shape:

```json
{
  "capture_id": "capture-from-current-observation",
  "options": {
    "kinds": ["text", "icon"],
    "min_confidence": 0.5,
    "max_regions": 100
  }
}
```

Region rectangles are half-open source-screenshot pixel bounds with a top-left
origin. Do not treat OCR text, an icon label, or an `interactive` hint as proof
that a region is actionable. Do not reuse a region after an action, resize,
move, scroll, navigation, display-layout change, or target-identity change.

## Handle failures by category

- `not_installed`: the optional `cua-perception` extension is absent. Continue with
  accessibility, browser, or caller-owned visual reasoning unless the user has
  chosen to inspect and install the preview.
- `capture_not_found`, `capture_expired`, `capture_stale`, or
  `capture_generation_mismatch`: take a fresh capture and parse that new ID.
- `unsupported_target` or `unsupported_platform`: use another observation
  route and preserve the reported limitation.
- `incompatible_protocol` or `artifact_invalid`: stop using the extension;
  inspect its exact installed state before any update or reinstall.
- `worker_launch_failed`, `worker_crashed`, `worker_cancelled`, `timeout`, or
  `inference_failed`: do not act from a partial or old result. Reobserve before
  a bounded retry or fallback.
- `invalid_frame` or `resource_limit_exceeded`: narrow the request or use a
  supported native-resolution capture; never clamp regions or silently change
  coordinates.

The worker parses only the admitted screenshot and bounded options. It does not
own desktop capture, accessibility, browser, input, credential-store, or action
authority.

Parsing a window does not make its pixel input background-safe. Preserve the
platform action ladder and exact `background_unavailable` refusal. Foreground
delivery and desktop input can change focus, workspace, or the system cursor;
use them only when already authorized for the workflow.

An external chooser, including a `jev-use` recipe, receives a bounded table of
opaque action IDs rather than open-ended Driver tool access. The caller keeps
provider setup and credentials outside Driver, validates one returned ID, and
dispatches the complete prebuilt action unchanged. See [Use jev-use with visual
regions](https://cua.ai/docs/how-to-guides/driver/use-jev-use-with-visual-regions);
do not add provider SDK logic or credentials to this skill.
