# Streaming example: Rust

The shared scenario ([`../SCENARIO.md`](../SCENARIO.md)) on the `cua-sdk`
crate, used directly as a Rust library: `Cua::embedded` → `Cua::env(url,
token)` → `SpacesdClient::open_media_decoded_with_audio` with `DecodedFrameSink` /
`PcmSink` callbacks (BGRA frames decoded by VideoToolbox on macOS, OpenH264
elsewhere; Opus decoded to PCM). The click goes over the media socket as an
`interactive_input` pointer batch through `MediaSession::send_control`.

It is headless only (counts and hashes frames, writes WAVs). For a rendered
window use `cua-viewer` (`libs/cua/crates/cua-viewer`), which is the Rust
viewer on the same media plane.

## Run

```sh
# A local Space (docker; add --runtime runsc for gVisor)
libs/cua/bench/streaming/scripts/space.sh start
eval "$(libs/cua/bench/streaming/scripts/space.sh env)"

cd examples/streaming/rust
CARGO_BUILD_JOBS=4 cargo build --release
CUA_STREAM_SECONDS=5 CUA_OUT_DIR=out ./target/release/cua-streaming-example
```

Prints the targets, then one `SUMMARY {...}` line; exit 0 when both streams
produced frames and the grid logged the click. Example (local runc Space,
macOS host):

```
SUMMARY {"click":{"frame_point":[200.0,280.0],"logged":true,"pixel":[71,153,130],"pixel_ok":true,"sent":true,"via":"interactive_input"},
 "desktop":{"audio_packets":21,"first_frame_ms":94.9,"frames":1,"last_hash":"ae1a0b47acc9e337","size":[1280,800],...},
 "window":{"audio_packets":19,"first_frame_ms":90.7,"frames":1,"last_hash":"21b28ed1ee0e169e","size":[640,480],...}}
```

Static targets are damage-driven: an idle desktop or grid produces about one
frame per 5 s, so `fps` is low by design. `bytes`/`keyframes` are `null`
because the decoded callbacks do not carry the encoded size or keyframe flag.

## Bench mode

With `CUA_BENCH_JSONL=<file>` (plus `CUA_BENCH_TARGET`, `CUA_BENCH_SECONDS`,
`CUA_BENCH_AUDIO`) it streams one target and writes the per-frame JSONL with
the decoded timecode; `libs/cua/bench/streaming/shims/rust-sdk.sh` is the
harness launcher (`--clients rust-sdk`).
