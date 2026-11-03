# Streaming example: Swift

The shared scenario ([`../SCENARIO.md`](../SCENARIO.md)) on the cua Swift SDK
(`libs/cua/swift`, `import Cua`): connect, start the grid fixture, list
targets, stream the desktop and the grid window (H.264 + Opus, decoded to
BGRA / PCM by the SDK), click grid cell (2, 3), print `SUMMARY {...}`. It is
also a bench "language lane" (`CUA_BENCH_JSONL`).

- **Headless** (default, `CUA_HEADLESS=1`): counts frames, hashes the last
  decoded frame (FNV-1a 64 over BGRA), writes `desktop.wav` / `window.wav`.
- **Window** (`CUA_HEADLESS=0`): the same run while an AppKit window shows
  the decoded frames and AVAudioEngine plays the audio. It opens a window on
  your desktop, so run it yourself; CI and the bench only use headless.

## Build

The package depends on `libs/cua/swift` by path and links the cua-sdk static
library through an XCFramework (the package's `CUA_SWIFT_XCFRAMEWORK` mode):

```sh
# 1. cargo build --release -p cua-sdk, then wrap target/release/libcua_sdk.a as
#    libs/cua/swift/build/CuaSDKFFI.xcframework (git-ignored there).
examples/streaming/swift/scripts/build-ffi.sh

# 2. Build the example (the path is relative to libs/cua/swift).
cd examples/streaming/swift
CUA_SWIFT_XCFRAMEWORK=build/CuaSDKFFI.xcframework swift build -c release
```

Command Line Tools are enough, no Xcode needed. The linker prints `object file
... was built for newer 'macOS' version` warnings for the Rust archive. They
are harmless.

Why not the dev dylib? `libs/cua/swift/lib/libcua_sdk.dylib` is checked in
and was older than the Rust sources when this was written: it lacks
`open_media_decoded*`, so linking against it fails. The XCFramework route
leaves tracked files alone.

`deps/cua-swift` is a symlink to `libs/cua/swift`. SwiftPM names a path
package after its directory, and both this package and the SDK live in a
directory called `swift`, so the symlink gives the dependency a different
identity.

## Run

```sh
libs/cua/bench/streaming/scripts/space.sh start
eval "$(libs/cua/bench/streaming/scripts/space.sh env)"
cd examples/streaming/swift
CUA_HEADLESS=1 CUA_STREAM_SECONDS=5 CUA_OUT_DIR=out .build/release/cua-streaming
# window mode (opens a window, plays audio):
CUA_HEADLESS=0 .build/release/cua-streaming
```

Bench lane (the harness sets these):

```sh
libs/cua/bench/streaming/scripts/space.sh fixture timecode
CUA_BENCH_JSONL=out/bench.jsonl CUA_BENCH_TARGET="window:CUA Bench Timecode" \
  CUA_BENCH_SECONDS=5 CUA_BENCH_AUDIO=1 .build/release/cua-streaming
```

`tc_ms` is decoded from every frame. On a `display:` target the strip is
found at the bench window's position from `ListTargets`.

## Notes

- **Click.** The example first sends a media-plane `action` (`tool: "click"`,
  pixel basis) on a session opened with
  `policy: SESSION_POLICY_ALLOW_ACTIVATION`. The default policy is view-only
  and the driver answers `view_only`. If the fixture log shows no press, it
  falls back to `ComputerService.Pointer` with `DELIVERY_FOREGROUND` at the
  window origin + (200, 280). The summary's `via` says which one landed.
  `env.click()` (auto delivery) picks X11 `XSendEvent`, which GTK3 ignores,
  so it is not used.
- **`keyframes` / `bytes` are `null`.** The decoded callbacks
  (`DecodedFrameSink`, `PcmSink`) carry neither the encoded size nor the
  keyframe flag. The bench JSONL has `"bytes":null,"key":null` for the same
  reason.
- **Summary extras.** `pcm_frames`, `frames_dropped`, `decode_errors`,
  `last_size`, `hash_of` and the click's `action_*` / `env_report` fields are
  extra keys beyond SCENARIO.md.
- **Stop the bench fixture before the scenario.** Its keep-above window
  covers grid cell (2, 3) (`space.sh fixture` starts it), so the click
  misses the grid.
- **Frame counts.** The driver sends frames on damage, so a static desktop
  gives about 1 frame in 5 s and `fps` is `null`.
