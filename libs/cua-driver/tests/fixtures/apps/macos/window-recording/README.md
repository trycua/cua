# Exact-window recording fixture

This AppKit fixture and `window_recording_macos_test` provide focused native
ScreenCaptureKit diagnostics. Prefer a disposable, logged-in Lume macOS 15+
worker prepared by the canonical macOS harness. An explicitly authorized host
run is also available with the isolation requirements below. This focused test
does not replace the complete harness certification.

Prerequisites: a clean, committed candidate and its signed, TCC-authorized
installed daemon, an unrestricted session, Rust, Swift, `ffmpeg`, and `ffprobe`.
Preserve the source SHA alongside the evidence. Commit intended changes before
installing; the test rejects the installer's `-dirty` provenance marker. Use the same installed
driver and daemon socket verified by the canonical Lume runner for VM runs.
Never start a raw unsigned daemon or use the released Driver for host diagnostics.

From the repository root **inside the disposable VM**, with a fresh task-owned
absolute evidence directory in `CUA_WINDOW_RECORDING_OUTPUT_ROOT`:

```bash
export CUA_WINDOW_RECORDING_DISPOSABLE_VM=1
export CUA_WINDOW_RECORDING_OUTPUT_ROOT=/tmp/window-recording-evidence-unique-run
mkdir -p "$CUA_WINDOW_RECORDING_OUTPUT_ROOT/build"
xcrun swiftc libs/cua-driver/tests/fixtures/apps/macos/window-recording/main.swift \
  -framework AppKit \
  -o "$CUA_WINDOW_RECORDING_OUTPUT_ROOT/build/window-recording-fixture"
export CUA_WINDOW_RECORDING_FIXTURE="$CUA_WINDOW_RECORDING_OUTPUT_ROOT/build/window-recording-fixture"
export CUA_TEST_DRIVER_BIN=/absolute/path/to/verified/installed/cua-driver
export CUA_E2E_MACOS_DAEMON_SOCKET=/absolute/path/to/verified/daemon.sock
export CUA_E2E_SOURCE_SHA=FULL_CANDIDATE_COMMIT_SHA
unset CUA_E2E_RECORDINGS_ROOT
cargo test --manifest-path libs/cua-driver/rust/Cargo.toml -p cua-driver \
  --test window_recording_macos_test -- --ignored --nocapture --test-threads=1
```

### Explicitly authorized host diagnostics

Obtain the host owner's explicit approval before this lane. Install the exact
candidate as the separate, certificate-signed `CuaDriverLocal.app`; preserve
the released Driver and its daemon. The host owner grants macOS permissions
through the normal app-owned prompt flow. Do not use VM-only TCC helpers.

Create a fresh absolute evidence root, then launch that local app in unrestricted
mode with `--dangerously-bypass-approvals` and an explicit socket at
`$CUA_WINDOW_RECORDING_OUTPUT_ROOT/driver.sock`. Do not enable autostart or point
other clients at this task-owned socket. Verify the app signature, installed
version, source SHA, socket owner, and daemon permission mode before proceeding.

Use the build and test commands above, but unset
`CUA_WINDOW_RECORDING_DISPOSABLE_VM` and set
`CUA_WINDOW_RECORDING_HOST_AUTHORIZED=1`. Set `CUA_TEST_DRIVER_BIN` to
`/Applications/CuaDriverLocal.app/Contents/MacOS/cua-driver-local` and
`CUA_E2E_MACOS_DAEMON_SOCKET` to that task-owned `driver.sock`. The test rejects
ambiguous opt-ins, default socket paths, other host CLI binary paths, and a
daemon whose version or source SHA differs from the candidate. This does not
attest the socket listener's executable or ownership: the operator must verify
those separately before the run. It also verifies that the
fixture-reported PID is the child it launched before requesting capture.

The fixture briefly activates and moves its own synthetic windows. Keep the
host idle during the run. Stop only the task-owned local daemon afterward;
leave the released Driver unchanged. `test-environment.json` labels the result
as host diagnostics, not canonical Lume certification.

`CUA_E2E_RECORDINGS_ROOT` must be unset: the generic testkit trajectory mode
records a full desktop and is deliberately forbidden in this test. The test
uses the existing `McpDriver` daemon proxy and public recording APIs with an
explicit `{kind: "window", pid, window_id}` on every start. Fixture stdin is
only an external control for changing its synthetic native windows; it is not
a capture implementation or substitute for native video proof.

The 321-by-241-point green target has a blue center marker and a pulsing white
bar. A larger red sibling starts visible beside it, then fully covers it before
both windows move. Every decoded frame must retain the target colors
without red contamination, and at least two frames must differ. Frame timestamps
must increase and span at least 1.5 seconds. The fixture acknowledges minimization
after AppKit completes the asynchronous transition, with a three-second deadline.
At launch, the fixture waits for three consecutive WindowServer samples matching
its intended borderless dimensions, with a five-second deadline, so a transient
expanded launch frame does not become the capture baseline.
Stream dimensions must match the fixture's native point dimensions and observed
display scale, rounded up to even physical pixels. Seven recordings cover three
explicit stops, minimize, close, resize, and owner disconnect. Each verifies native finalization,
the termination reason, a decoded playable MP4, and exactly `recording.mp4`
plus `session.json`. The same daemon also rejects wrong-PID and busy starts,
and starts subsequent recordings after finalization. Starting capture and
explicitly stopping or disconnecting must preserve the foreground process,
fixture window order, key window, and physical cursor position. The test samples
the attested daemon's open descriptor count before capture and after each
finalized recording. The first `stop` case warms the native capture frameworks;
subsequent cases permit at most two additional descriptors above that warmed
sample. This detects repeated growth, not a one-time resource retained at warmup.

Artifacts remain in `stop/`, `repeat_1/`, `repeat_2/`, `minimize/`, `close/`,
`resize/`, and `disconnect/`; there is no bulk cleanup. The root
`native-diagnostics.json` records descriptor counts and observed native scales,
not descriptor paths or other application state. An existing case path or report is rejected before
connecting, and the environment report is created without overwrite.
The fixture child is killed and reaped on normal return or Rust assertion
unwind, including an invalid or missing initial reply. The test does not install software, grant permissions, change desktop
settings, capture the desktop, or enable legacy trajectory recording.

Only the scales reported in `native-diagnostics.json` are exercised natively;
a scale-1 run does not certify Retina capture. Shared geometry tests separately
cover scale arithmetic and scale-change policy. This lane does not prove
display-scale changes, display reconfiguration,
cross-process occlusion, session ownership races, permission revocation, or
encoder failure recovery. Native execution is required before reporting the
test as passing; successful compilation alone proves no GUI behavior.
