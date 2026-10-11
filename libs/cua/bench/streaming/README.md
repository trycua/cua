# cua-bench-streaming

Benchmarks for the cua-spacesd media plane (rcdp wire v2: WebSocket
`/media` and direct QUIC, H.264 + Opus). A Rust driver runs a matrix of
runtime × transport × client decoder × scenario against local Spaces
containers, plus language lanes that drive the streaming examples
(`examples/streaming/*`) through `shims/*.sh`. Output is JSON plus a markdown
table, and `check` gates results against `budgets.json`.

## What is measured

| Metric | How |
|---|---|
| glass-to-glass (`g2g_ms_*`) | The fixture (`fixtures/benchfix.py`) draws a checksummed wall-clock timecode strip into every frame. The client decodes the frame, reads the strip and subtracts it from its own clock, corrected by the guest/host offset measured against the fixture's TCP time server (min-RTT of 60 pings, error ≤ RTT/2, typically < 0.5 ms). |
| input → photon (`input_photon_ms_*`, `action_photon_ms_*`) | Every 1.5 s the client clicks the fixture's photon square and times until a decoded frame shows it toggled. Odd probes use `interactive_input` (pointer move/down/up), even probes use a media-plane `action` (`click`, pixel basis), which also yields `action_result_ms` and `ActionFrameCorrelation` timing (`action_correlation_ms`, `correlated_to_photon_frames`). |
| time to first frame (`ttff_ms`) | `OpenMedia` call to the first decoded frame (`open_ms` is the RPC + attach part). |
| fps stability (`fps`, `interval_ms_p50/p95/max`) | Decoded-frame intervals. |
| bytes/s (`video_bytes_per_s`, `wire_bytes_per_s`, `audio_bytes_per_s`) | Payload bytes per scenario: static, desktop-static, timecode (small damage), scroll (text), video (moving blocks + gradient), drag (window moving along a circle, desktop target). |
| keyframe recovery (`recovery_ms_*`) | `loss`: a fraction of QUIC *video* datagrams is discarded at the client before reassembly (seeded, default 5 %); recovery is the time from reference loss to the next decoded frame, with keyframe requests re-sent at most 1/s while waiting. `stall`: the WebSocket reader stops for 1 s every ~5 s; recovery is the time from resuming to the first fresh frame (g2g < 200 ms). |
| CPU (`client_cpu_pct`, `server_cpu_pct`, `container_cpu_pct`) | Client: `getrusage(RUSAGE_SELF)` (language lanes: `RUSAGE_CHILDREN` of the shim process tree). Server: cua-spacesd `utime+stime` from the guest's `/proc`, read through the driver's own process API. Container: the docker cgroup's CPU as the host sees it (for runsc this includes the gVisor sandbox). Percent of one core. |
| audio latency, A/V skew (`audio_latency_ms_*`, `av_skew_ms_*`, `flash_latency_ms_*`) | `avsync`: the image's A/V fixture flashes and beeps (1 kHz) on every wall-clock second. The client detects beep onsets in decoded PCM (Goertzel) and flash frames (mean luma); audio latency is onset arrival vs the second boundary (offset-corrected), skew is flash `capture_timestamp_us` minus beep onset `pts_us` on the shared media clock. Budget ±40 ms (plan §8.5). |
| decode (`decode_us_*`) | Per access unit, per backend (`videotoolbox`, `openh264`). |
| server stats | `GetStats` at the end: encoder, frames dropped/replaced, keyframes sent, keyframe requests, bitrate. |

Hardware encoders (NVENC, VA-API, QSV, AMF, MediaFoundation) are listed as
"deferred" in every result (plan §8.6): they are probe-only until GPU access.
The server encoder in the Linux container is OpenH264.

## Lanes

- **Native** (`--clients rust`): this binary on the host. macOS host lanes
  decode with VideoToolbox by default (`--alt-decoders openh264` adds OpenH264
  comparison runs).
- **Sidecar** (`--sidecar`): the Linux build of this binary
  (`scripts/build-linux.sh`) in a container sharing the Space's network
  namespace (`--network container:<space>`), OpenH264 decode, WS + QUIC. On
  macOS, Docker (colima / Docker Desktop) does not forward UDP from the host
  into containers, so QUIC lanes run here; pass `--host-quic false`.
- **Language** (`--clients node,python,browser,rust-sdk`): the examples in
  bench mode (`examples/streaming/SCENARIO.md`, "Benchmark JSONL"), timecode
  scenario over WebSocket, first runtime only. A shim exiting 3 means the
  lane's prerequisites are missing; it is recorded under "Not run".
- **Runtime**: each `--runtimes` entry (`runc`, `runsc`) starts a fresh
  container (`scripts/space.sh`, `--memory=4g`, one at a time).
  `--attach NAME` uses a running container, `--env-url` any spacesd
  (with `--quic-addr`, `--time-addr`, token in `CUA_ENV_TOKEN`).

## Run

```sh
eval "$(/opt/homebrew/bin/brew shellenv)"; source ~/.cargo/env
cd libs/cua/bench/streaming
cargo build --release
# Optional: benchmark the driver built from this commit instead of the image's
CUA_BENCH_DRIVER_BIN=$PWD/../../../images/linux/dist/arm64/cua-spacesd \
./target/release/cua-bench-streaming run --runtimes runc,runsc --seconds 20 \
  --transports ws,quic --host-quic false --sidecar --alt-decoders openh264 \
  --clients rust,rust-sdk,node,python,browser \
  --out results/latest.json --markdown results/latest.md --check
./target/release/cua-bench-streaming check --results results/baseline-macos-arm64.json
./target/release/cua-bench-streaming report --results results/baseline-macos-arm64.json
```

On Linux (CI) QUIC reaches the published UDP port directly, so the native
lanes cover both transports and no sidecar is needed.

Memory safety: every media loop is bounded (≤ 2 M items per run, bounded
channels, bounded shim output), runs are time-boxed, containers get
`--memory=4g`.

## Files

- `src/`: `link.rs` (gRPC + WS/QUIC media link, loss injection), `video.rs`
  (decoder per backend, timecode/photon reader), `audio.rs` (Opus decode,
  onset detection), `run.rs` (one native run), `shim.rs` (language lanes),
  `space.rs` (Space control through the spacesd process API, clock offset),
  `report.rs` (schema, markdown, budget gate).
- `fixtures/benchfix.py`: the self-describing GTK fixture (uploaded into the
  guest at run time; the image is not modified).
- `scripts/space.sh`: local container lifecycle; `scripts/build-linux.sh`:
  Linux build for the sidecar.
- `budgets.json`: regression thresholds; `results/`: committed baselines.
