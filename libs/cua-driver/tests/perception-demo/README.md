# Visual perception demo evidence

This directory defines the review-only evidence boundary for the visual canvas
demo. The GitHub workflow runs mock and static checks, then runs the canonical
Windows and Linux X11 Driver harnesses without secrets. A protected job consumes
an immutable review-candidate aggregate, runs the ignored visual-only test with
the measured review Driver, and makes the API key available only to the bounded
Jev chooser process. Evidence upload is limited to the redacted manifest and
decoded MP4 described below.

The candidate aggregate has `windows` and `linux-x11` directories. Each
directory contains `signed-catalog.json`, the catalog-selected extension
archive, `review-measurements.json`, `signed-candidate-checksums.txt`, and a
`review-cua-driver` binary (`review-cua-driver.exe` on Windows). The measurement
file binds the review-only source, debug review-trust-root build profile,
Ed25519 public key, signed catalog, extension archive, supplied model, and
Driver binary by SHA-256. The test requires these environment variables:

- `CUA_JEV_MOCK_DEMO=1`, or `CUA_JEV_LIVE=1` with
  `CUA_JEV_CHOOSER_PROGRAM`, `CUA_JEV_CHOOSER_SCRIPT`, and `TYPESAFE_API_KEY`
- `CUA_E2E_SOURCE_SHA`
- `CUA_JEV_SOURCE_SHA`
- `CUA_SESSION_LABEL`
- `CUA_TEST_DRIVER_BIN`
- `CUA_CANDIDATE_MEASUREMENTS`
- `CUA_PERCEPTION_MODEL`
- `CUA_PERCEPTION_EXTENSION_HOME`
- `CUA_PERCEPTION_EVIDENCE_DIR`
- `CUA_RUNNER_OS_NAME`
- `CUA_RUNNER_OS_VERSION`
- `CUA_RUNNER_OS_ARCH`

The extension home must already contain the candidate installed by the Driver's
signed extension lifecycle. The Driver verifies the signed Ed25519 catalog
against the embedded review-only trust root and reports
`review-only-publisher-verified` before launching the fixture. The
workflow also checks the aggregate checksum file and every measured artifact
hash before executing the review Driver. The test writes measured
`raw-manifest.json` and `timeline.json` files beneath the evidence directory.
It also copies the decoded testkit clip to `recording.mp4` and writes the
schema-checked, redacted `manifest.json`. The workflow uploads only
`recording.mp4` and `manifest.json`; capture IDs, coordinates, local paths, and
the raw timeline remain runner-local.

`sanitize_evidence.py` measures the checked-out source SHA and host platform,
reads the fixture's loopback oracle and adapter result, hashes the model,
Driver, capture identifiers, and recording bytes, and binds the signed catalog,
extension archive, and Ed25519 signing-key measurements. Its output is limited
to `manifest.json` and `recording.mp4`.

macOS is intentionally separate from this Windows/Linux workflow. Native macOS
proof uses the logged-in, TCC-authorized Lume runner and the canonical
`libs/cua-driver/tests/runners/macos-lume/run-all.sh --standalone-browser`
harness with a separately assembled and signed arm64 candidate.
