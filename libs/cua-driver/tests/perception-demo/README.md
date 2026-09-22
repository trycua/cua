# Visual perception demo evidence

This directory defines the review-only evidence boundary for the visual canvas
demo. Independent exact-SHA workflows run and certify the canonical Windows,
Linux X11, and logged-in macOS Lume Driver harnesses without live-provider
secrets. The protected workflows verify those run IDs and their machine-readable
certification artifacts instead of rerunning the broad matrices. A protected
job consumes an immutable review-candidate aggregate, runs the ignored visual-only test with
the measured review Driver, and makes the API key available only to the bounded
Jev chooser process. After decode and schema validation, each review bundle is
encrypted separately and the artifact upload contains only the resulting
authenticated ciphertext envelopes. Manual and reusable workflow invocations
must also set the required boolean `run_live` acknowledgement to `true` before
the source gate permits protected work.

The private candidate producer runs without an environment or production
secrets on Linux, Windows, and `macos-26`, then returns both its Actions run ID
and the immutable aggregate artifact ID. Consumers require both identifiers so
the artifact download is bound to the reviewed exact-SHA producer run. The
candidate aggregate has `windows`, `linux-x11`, and `macos` directories. Each
directory contains `signed-catalog.json`, the catalog-selected extension
archive, `review-measurements.json`, `signed-candidate-checksums.txt`, and a
`review-cua-driver` binary (`review-cua-driver.exe` on Windows). The measurement
file binds the review-only source, Driver version and debug review-trust-root
build profile, Ed25519 public key, signed catalog, extension archive, worker,
all three model artifacts, ONNX Runtime, executed self-test, Driver binary, and
an explicit platform code-signing measurement. Linux and Windows record Apple
code signing as not applicable. The macOS arm64 Driver is signed before it is
measured with a temporary self-signed review certificate in a temporary
keychain; the keychain and private key are destroyed before artifact upload.
Its measurement records the certificate hash and designated requirement and
does not claim a production identity.
Those extension measurements are extracted from the sealed archive and checked
against its artifact manifest, model ledger, runtime contract, and bytes rather
than copied from workflow constants. The test requires these environment variables:

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
- `CUA_DESKTOP_SESSION_TYPE`
- `CUA_RUNNER_IDENTITY_CLASS`

The extension home must already contain the candidate installed by the Driver's
signed extension lifecycle. The Driver verifies the signed Ed25519 catalog
against the embedded review-only trust root and reports
`review-only-publisher-verified` before launching the fixture. The
workflow also checks the aggregate checksum file and every measured artifact
hash before executing the review Driver. The test writes measured
`raw-manifest.json` and `timeline.json` files beneath the evidence directory.
It also copies the decoded testkit clip to `recording.mp4` and writes the
schema-checked, redacted `manifest.json`. Capture IDs, coordinates, local paths,
the raw timeline, and the plaintext review bundle remain runner-local.

`sanitize_evidence.py` measures the checked-out source SHA and host platform,
reads the fixture's loopback oracle and provider-returned adapter result, hashes
the Driver, private capture trace, and recording bytes, and binds the sealed
worker, model, runtime, protocol, and self-test measurements. The redacted
manifest records safe bounded candidate IDs and descriptions, capture source
and dimensions, session and runner class, delivery mode, and FFprobe
measured resolution, frame rate, uncut 1x edit record, source time range, cursor
configuration, and final hash. Its output is limited to `manifest.json` and
`recording.mp4`.

The live acceptance test keeps two distinct rows: native-resolution window
capture with background delivery, and primary-desktop capture with foreground
delivery. Primary-desktop actions are screen-absolute on all three supported
desktop families, so background delivery is not supported for that row. The
desktop row writes private evidence beneath `primary-desktop/`; the existing
window evidence remains at the evidence root. Validation stages independently
sanitize and fully decode both rows into review-safe `window/` and
`primary-desktop/` leaves, each containing only `manifest.json` and
`recording.mp4`. `evidence_envelope.py` then validates each exact two-file
directory again and encrypts it for an X25519 recipient. Every unreleased v3
envelope creates a fresh ephemeral X25519 key and derives a one-time
AES-256-GCM key with HKDF-SHA256. The intended recipient public key's SHA-256
fingerprint is included in both the KDF context and authenticated versioned
header. The header also contains the ephemeral public key, nonce, algorithm,
flags, and ciphertext length. Local
decryption derives the public key from the supplied private key and rejects a
recipient fingerprint mismatch before attempting decryption. Encryption logs
only the safe recipient fingerprint, and the same fingerprint remains in each
envelope so protected-run logs and downloaded artifacts identify the key used.
The protected GitHub
environment supplies only the canonical-base64 recipient public key through the
`EVIDENCE_ARCHIVE_RECIPIENT_PUBLIC_KEY` environment variable; this is a GitHub
environment variable, not a secret, and the recipient private key must never be
stored in GitHub or provided to a runner. The uploaded directory contains
exactly `window.cuae` and `primary-desktop.cuae`; it never contains plaintext
manifests, recordings, raw evidence, or timelines. Reviewers decrypt locally
with the canonical-base64
private X25519 key, for example `evidence_envelope.py decrypt --input SCOPE.cuae
--output OUTPUT --private-key-file PRIVATE_KEY_FILE`; `--private-key-env` is
also supported for a local environment variable. Encryption accepts the public
recipient through `--recipient-env` or `--recipient-file`. Windows and Linux
keep raw, mock, and validated plaintext evidence beneath the runner's temporary
directory and remove those exact directories in an `always()` step after the
upload step.

Native macOS live evidence remains separate from the Windows/Linux workflow.
This proof uses the logged-in, TCC-authorized Lume runner. A maintainer first
dispatches `.github/workflows/e2e-rust-macos.yml` in `lume` mode, whose exact-SHA gate runs
`libs/cua-driver/tests/runners/macos-lume/run-all.sh --standalone-browser` and
publishes a certification artifact without live-provider credentials. The
protected evidence workflow then attests that run before using the `macos`
member of the same private review aggregate. The
labeled PR workflow produces the candidate and runs only on GitHub-hosted
Linux, Windows, and macOS machines; the pull-request event never schedules the
self-hosted Lume runner. After that producer run completes successfully, a
maintainer manually dispatches `authorized-live-jev-macos-evidence.yml` with
its immutable producer run and artifact IDs. This keeps public pull-request
events from directly scheduling work on the self-hosted Lume runner. The manual
path requires a protected,
console-logged-in self-hosted Lume runner. It verifies the exact current source
and Jev heads, producer run ID, aggregate artifact ID, the candidate Driver's
certificate-backed arm64 signature and hash, aggregate checksums, the Ed25519
catalog signature and measured public key, the signed extension identity, and
the same redacted manifest contract;
it fully decodes the recording before encrypting the two scope bundles and
uploading only their `.cuae` envelopes. Recording, raw evidence, sanitizer
inputs, and validated plaintext directories live beneath the runner's temporary
directory and are removed by an `always()` cleanup step after the upload step.

## Derived reels

Raw evidence remains an uncut 1x record. A shorter review reel is a distinct
artifact governed by `derived-reel-manifest.schema.json` and
`sanitize_derived_reel.py`. Its edit plan names safe source-relative evidence
manifests and recordings, at least two 1x source ranges, and every wait-only
interval removed between ranges from the same source. Each wait cut must state
exactly `wait-only interval removed; no action or result omitted`; the tool
rejects missing, extra, or differently described gaps.

The sanitizer rehashes every source manifest, source recording, and final reel,
requires all source manifests to be passed live evidence for the same Driver
and Jev SHAs, bounds each range by FFprobe's measured source duration, fully
decodes the reel, and checks its duration against the declared edit timeline.
It publishes only `manifest.json` and `reel.mp4`. The derived manifest records
per-source hashes, used ranges, disclosed wait ranges, real trim and concatenate
operations, the expected and decoded durations, and the final hash. Local paths
and the edit plan are not published.

Example edit-plan shape:

```json
{
  "schema": "cua-derived-reel-edit-plan/v1",
  "sources": [
    {"id": "macos", "evidence_manifest": "macos/manifest.json", "recording": "macos/recording.mp4"}
  ],
  "shots": [
    {"source_id": "macos", "start_ms": 500, "end_ms": 1500},
    {"source_id": "macos", "start_ms": 3000, "end_ms": 4000}
  ],
  "wait_cuts": [
    {"source_id": "macos", "start_ms": 1500, "end_ms": 3000, "disclosure": "wait-only interval removed; no action or result omitted"}
  ]
}
```

Render the reel from those exact ranges with FFmpeg, then validate and stage it:

```text
python3 sanitize_derived_reel.py --source-root INPUTS --edit-plan edit-plan.json \
  --reel derived.mp4 --output-dir publish-reel
```
