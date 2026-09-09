# Exact-window recording fixture

This AppKit fixture and `window_recording_macos_test` provide focused native
ScreenCaptureKit diagnostics. Run only in a disposable, logged-in Lume macOS
15+ worker prepared by the canonical macOS harness. This focused test does not
replace the complete harness certification.

Prerequisites: the exact candidate source and signed, TCC-authorized installed
daemon, its unrestricted session, Rust, Swift, `ffmpeg`, and `ffprobe`. Preserve
the source SHA and dirty diff alongside the evidence. Use the same installed
driver and daemon socket verified by the canonical Lume runner. Do not run on
the developer's desktop or start a separate raw unsigned daemon.

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
unset CUA_E2E_RECORDINGS_ROOT
cargo test --manifest-path libs/cua-driver/rust/Cargo.toml -p cua-driver \
  --test window_recording_macos_test -- --ignored --nocapture --test-threads=1
```

`CUA_E2E_RECORDINGS_ROOT` must be unset: the generic testkit trajectory mode
records a full desktop and is deliberately forbidden in this test. The test
uses the existing `McpDriver` daemon proxy and public recording APIs with an
explicit `{kind: "window", pid, window_id}` on every start. Fixture stdin is
only an external control for changing its synthetic native windows; it is not
a capture implementation or substitute for native video proof.

The green target has a blue center marker and a pulsing white bar. A larger red sibling fully covers
it before both windows move. Every decoded half-second sample must retain the
target colors without red contamination, and at least two samples must differ.
Stream dimensions must match the fixture's native point dimensions and display
scale. Five recordings cover explicit stop, minimize, close, resize, and owner
disconnect. Each verifies native finalization,
the termination reason, a decoded playable MP4, and exactly `recording.mp4`
plus `session.json`. The same daemon also rejects wrong-PID and busy starts,
and starts subsequent recordings after finalization.

Artifacts remain in `stop/`, `minimize/`, `close/`, `resize/`, and `disconnect/`; there is no
bulk cleanup. Reusing an existing case directory fails to preserve evidence.
The fixture child is killed and reaped on normal return or Rust assertion
unwind. The test does not install software, grant permissions, change desktop
settings, capture the desktop, or enable legacy trajectory recording.

This lane does not prove display-scale changes, display reconfiguration,
cross-process occlusion, session ownership races, permission revocation, or
encoder failure recovery. Native execution is required before reporting the
test as passing; successful compilation alone proves no GUI behavior.
