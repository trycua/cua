# Streaming example: Python

Implements [`../SCENARIO.md`](../SCENARIO.md) with the `cua` Python binding
(`libs/cua/python`, UniFFI over the Rust `cua-sdk` crate). Standard library
only; the SDK decodes video (VideoToolbox on macOS, OpenH264 elsewhere) and
Opus audio, so the example receives BGRA frames and s16 PCM through
`SpacesdClient.open_media_decoded_with_audio(options, DecodedFrameSink, PcmSink)`.

## Setup (uv, Python 3.12)

From the repository root:

```sh
examples/streaming/python/setup.sh            # cargo build --release -p cua-sdk + venv
examples/streaming/python/setup.sh --no-build # reuse libs/cua/target/release/libcua_sdk.*
```

`setup.sh` creates `examples/streaming/python/.venv`, installs
`libs/cua/python` into it (non-editable, no deps) and copies the freshly
built `libcua_sdk` library next to the installed `cua/_native.py`. It honours
`CARGO_TARGET_DIR` (default `libs/cua/target`) and `CARGO_BUILD_JOBS`
(default 4).

## Run (headless)

```sh
libs/cua/bench/streaming/scripts/space.sh start
eval "$(libs/cua/bench/streaming/scripts/space.sh env)"
CUA_HEADLESS=1 CUA_STREAM_SECONDS=5 CUA_OUT_DIR=/tmp/cua-py-out \
    examples/streaming/python/.venv/bin/python examples/streaming/python/stream_example.py
libs/cua/bench/streaming/scripts/space.sh stop
```

It prints the driver health, one line per target, per-stream stats, and a
final `SUMMARY {...}` line, and writes `desktop.wav` / `window.wav` to
`CUA_OUT_DIR` (default `./out`). Exit code 0 only when both streams produced
frames and the click was logged by the grid fixture.

`CUA_HEADLESS=0` renders frames with tkinter (no audio playback: the
standard library has no audio output). Without tkinter it runs headless.

## Bench lane

`libs/cua/bench/streaming/shims/python.sh` runs the example in bench mode
from the venv (exits 3 with build instructions when the venv is missing):

```sh
libs/cua/bench/streaming/scripts/space.sh fixture timecode
CUA_BENCH_JSONL=/tmp/py.jsonl CUA_BENCH_TARGET='window:CUA Bench Timecode' \
CUA_BENCH_SECONDS=5 CUA_BENCH_AUDIO=0 libs/cua/bench/streaming/shims/python.sh
```

`tc_ms` is decoded from the fixture's 48-cell strip on every decoded frame
(syncs + checksum checked, 2³² window chosen nearest to the client clock).

## Notes and SDK gaps

- The decoded callbacks do not carry the encoded access unit's size or
  keyframe flag (`DecodedVideoFrame` has neither), nor the encoded audio
  packet size (`PcmAudio`). So `keyframes`/`bytes` in the summary and
  `key`/`bytes` in the bench JSONL are `null`. `encoded_frames` /
  `audio_packets` come from `MediaSession.stats()`.
- `last_hash` is FNV-1a 64 over the last decoded BGRA frame (`hash_of`).
- `StreamService.ListTargets` has no typed wrapper; it goes through
  `SpacesdClient.call_json`.
- Media sessions are view-only by default (`action_result.error.code =
  view_only`), so the window session is opened with
  `MediaOpenOptions.request_json = {"policy": "SESSION_POLICY_ALLOW_ACTIVATION"}`.
  `MediaOpenOptions` has no typed policy field.
- The click is a media-plane `action` via `MediaSession.send_control`;
  if no delivered `action_result` arrives within 3 s the example falls back
  to `SpacesdClient.click` in screen coordinates (`click.via` says which).
