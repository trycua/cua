# Streaming example: TypeScript on Node

The shared scenario ([`../SCENARIO.md`](../SCENARIO.md)) on `@trycua/cua` for
Node. It runs as TypeScript directly: Node 23.6 and later strip the types, so
there is no build step. It has been tested on Node 26.

What it uses from the SDK:

- `embedded().spacesd(url, token)`: a direct `SpacesdClient`. It calls `health`,
  `sh` (to run `cua-fixtures start grid` and read the fixture log),
  `callJson("StreamService/ListTargets")` and `click` (the fallback path).
- `openMediaDecodedWithAudio`: this is the default media path. The SDK
  decodes on its own delivery thread: H.264 goes to packed BGRA through
  VideoToolbox on macOS and OpenH264 elsewhere, and Opus goes to s16 PCM.
  The example hashes the last frame (FNV-1a 64 over BGRA), reads the bench
  timecode (`tc_ms`), checks the clicked pixel and writes the WAV files.
- `openMediaWithAudio` (`CUA_NODE_MEDIA=encoded`): this path delivers raw
  access units and Opus packets. It gives exact `bytes` and `keyframes` per
  frame, but there are no pixels, so `tc_ms` is null, and there is no WAV.
  `last_hash` is then the hash of the last access unit.
- `MediaSession.sendControl` sends the click as a media-plane `action` with a
  pixel basis (the current `geometry_epoch` and the last frame sequence). The
  window session is opened with `policy: SESSION_POLICY_ALLOW_ACTIVATION`
  through `MediaOpenOptions.requestJson`, because the default view-only policy
  refuses input. If the action is not delivered, the example falls back to
  `env.click` in screen coordinates (window origin + (200, 280)).

Limitations:

- The decoded callbacks (`DecodedVideoFrame`, `PcmAudio`) do not carry the
  access-unit size, the keyframe flag or the Opus packet size. In the default
  mode, the summary's `keyframes`/`bytes` and the bench lines'
  `bytes`/`key` are therefore `null`. Use `CUA_NODE_MEDIA=encoded` when you
  need them.
- Node has no renderer, so `CUA_HEADLESS=0` is ignored and the example always
  runs headless.

## Build the SDK (once)

Run these from the repository root:

```sh
cd libs/cua
CARGO_BUILD_JOBS=4 cargo build --release -p cua-sdk     # features default to media + media-decode
(cd typescript && npm ci && npm run build)
# Stage libcua_sdk + the N-API runtime as the local @trycua/cua-<triple>
# package. Do not set CARGO_TARGET_DIR for this step (see "Known issues").
env -u CARGO_TARGET_DIR node scripts/stage-uniffi-library.mjs --only=node
cd ../../examples/streaming/typescript-node && npm install   # links @trycua/cua (file:)
```

## Run it headless

```sh
S=libs/cua/bench/streaming/scripts/space.sh
$S start && eval "$($S env)"
(cd examples/streaming/typescript-node && CUA_OUT_DIR=./out node main.ts)
$S stop
```

The last line of output is `SUMMARY {...}`. The WAV files go to
`$CUA_OUT_DIR/desktop.wav` and `$CUA_OUT_DIR/window.wav`. The exit code is 0
only when both streams produced frames and the fixture logged the click.

## Bench mode

```sh
$S fixture timecode
CUA_BENCH_JSONL=/tmp/node.jsonl CUA_BENCH_TARGET='window:CUA Bench Timecode' \
CUA_BENCH_SECONDS=5 CUA_BENCH_AUDIO=1 libs/cua/bench/streaming/shims/node.sh
```

This appends `open`, `frame` (with `tc_ms`), `audio` and `end` lines. In
`end`, the CPU times come from `process.resourceUsage()`, and they cover the
whole Node process, including the SDK's Rust threads and decoders. The shim
exits 2 when the harness environment is missing and 3 (`skipped: …`) when a
build step has not been done.

`node --test pixels.test.ts` unit-tests the FNV, timecode and WAV helpers
without the native library.

## Known issues (SDK and driver)

- `libs/cua/scripts/build-node-runtime.mjs` fails with "missing built N-API
  runtime under …/runtime/napi/target/release" when `CARGO_TARGET_DIR` is set.
  Cargo then writes to that directory, but the script looks in the temporary
  crate's own `target/`. Unset it for staging.
- The click is not observed on the `linux` Space image. The
  media-plane `action` returns `action_result{delivered: true}` and
  `env.click`/`env.moveTo` return OK, but the pointer does not move and the
  grid fixture logs no event. `xdotool` inside the container on the same
  `DISPLAY=:1` works. So the summary reports `click.logged: false` and the
  exit code is 1.
