# Visual perception demo evidence

This directory defines the review-only evidence boundary for the visual canvas
demo. The GitHub workflow runs mock/static checks and the canonical Windows and
Linux X11 Driver harnesses. When an immutable signed-candidate artifact ID and
approved publisher-key digest are supplied, each platform also installs that
candidate and runs the ignored visual-only mock-choice E2E. It has no protected
environment, API key, or live Jev call. Evidence upload is limited to the
redacted manifest and decoded MP4 described below.

The candidate artifact contains `catalog.json`, `publisher.pem`, and platform
directories named `windows` and `linux-x11`. Each platform directory contains
`cua-perception.tar.gz`, its detached `cua-perception.tar.gz.sig`, and
`model.onnx`. The test requires these environment variables:

- `CUA_JEV_MOCK_DEMO=1`
- `CUA_E2E_SOURCE_SHA`
- `CUA_PERCEPTION_EXTENSION_ARCHIVE`
- `CUA_PERCEPTION_EXTENSION_SIGNATURE`
- `CUA_PERCEPTION_TRUSTED_PUBLIC_KEY`
- `CUA_PERCEPTION_TRUSTED_PUBLIC_KEY_SHA256`
- `CUA_PERCEPTION_MODEL`
- `CUA_PERCEPTION_EXTENSION_HOME`
- `CUA_PERCEPTION_EVIDENCE_DIR`

The extension home must already contain the candidate installed by the Driver's
signed extension lifecycle. The test independently verifies the detached
RSA-SHA256 signature before launching the fixture or Driver. It writes measured
`raw-manifest.json` and `timeline.json` files beneath the evidence directory.
It also copies the decoded testkit clip to `recording.mp4` and writes the
schema-checked, redacted `manifest.json`. The workflow uploads only
`recording.mp4` and `manifest.json`; capture IDs, coordinates, local paths, and
the raw timeline remain runner-local.

`sanitize_evidence.py` measures the checked-out source SHA and host platform,
reads the fixture's loopback oracle and adapter result, hashes the model,
extension, and recording bytes, and verifies the extension's detached
RSA-SHA256 signature against a public key whose digest is independently
approved. Its output is limited to `manifest.json` and `recording.mp4`.

macOS is intentionally separate from this Windows/Linux workflow. Native macOS
proof must use the logged-in, TCC-authorized Lume runner and the canonical
`libs/cua-driver/tests/runners/macos-lume/run-all.sh --standalone-browser`
harness before a macOS demo lane is added.
