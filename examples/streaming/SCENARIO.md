# Streaming examples: the shared scenario

Every example in this directory (`rust`, `swift`, `python`, `typescript-node`,
`typescript-web`, `kotlin`) runs the same script against a cua-spacesd, so
their output can be compared line by line and the benchmark harness
(`libs/cua/bench/streaming`) can drive any of them as a "language lane".

## Inputs (environment)

| Variable | Meaning | Default |
|---|---|---|
| `CUA_ENV_URL` | spacesd base URL | `http://127.0.0.1:33211` |
| `CUA_ENV_TOKEN` | env token (sent as `authorization: Bearer …`) | required |
| `CUA_STREAM_SECONDS` | seconds per streaming step | `5` |
| `CUA_HEADLESS` | `1`: no window/speaker; count and hash frames, write WAV | `1` in CI |
| `CUA_OUT_DIR` | where headless mode writes `desktop.wav` / `window.wav` | `./out` |
| `CUA_BENCH_JSONL` | if set, a file path: also write the per-frame JSONL below | unset |

A local Space for all of this:

```sh
libs/cua/bench/streaming/scripts/space.sh start            # runc; --runtime runsc for gVisor
eval "$(libs/cua/bench/streaming/scripts/space.sh env)"
# ... run an example ...
libs/cua/bench/streaming/scripts/space.sh stop
```

## Steps

1. **Connect** to `CUA_ENV_URL` with the token. Print the driver's health.
2. **Start the grid fixture** in the guest: run `cua-fixtures start grid`
   through the env process API (`sh`/`run`). It is idempotent.
3. **List targets** (`StreamService.ListTargets`, windows included). Print one
   line per target: kind (`display`/`window`), id, title, size, available.
   Pick the primary display and the window titled `CUA Fixture Grid`.
4. **Stream the desktop** for `CUA_STREAM_SECONDS`: H.264, `max_fps` 30,
   audio on (Opus). Render it (window mode) or, headless, count frames,
   keyframes and bytes, record time-to-first-frame, hash the last decoded
   frame (FNV-1a 64 over BGRA; examples that only see encoded access units
   hash the last access unit instead and say so), and write the decoded
   audio to `desktop.wav` (48 kHz s16le, channels as negotiated).
5. **Stream the grid window** the same way (`window.wav`).
6. **Click** grid cell (col 2, row 3) while the window stream is open. Its
   centre is content pixel (200, 280); scale it by
   `frame_width / window_width` when the frame is scaled. Send it over the
   media socket where the SDK exposes it (`MediaSession.send_control`), on a
   session opened with policy `SESSION_POLICY_ALLOW_ACTIVATION`:
   - preferred: an `interactive_input` batch of pointer `move`, `down`, `up`
     with `x_normalized = x / frame_width`, `y_normalized = y / frame_height`;
   - or a media-plane `action` (`tool: "click"`, `arguments: {x, y}`, pixel
     basis with the current `geometry_epoch` and last frame sequence). On the
     Linux image as of this writing the driver answers `delivered: true` but
     no X event arrives (known driver bug), so examples that use `action`
     fall back to the env pointer API with `DELIVERY_FOREGROUND` (XTest).
     Plain env `click` (auto delivery) uses XSendEvent, which GTK3 ignores.
   Stop the bench fixture first (`pkill -f benchfix`): its window is
   keep-above and covers the grid cell. Verify it two ways:
   - the fixture log `/tmp/cua-fixtures/grid.jsonl` (read through the env
     process API) has a `button_press` with `"cell": [2, 3]` after the click;
   - the decoded pixel at the click point is the cell colour
     `(r, g, b) = (72, 153, 128)` within ±24 per channel.
7. **Print a summary** as one JSON line prefixed with `SUMMARY `:

```json
{"example":"python","desktop":{"frames":151,"keyframes":1,"bytes":812345,
 "first_frame_ms":412.0,"fps":30.1,"audio_packets":250,"last_hash":"9f…",
 "wav":"out/desktop.wav"},
 "window":{...same...},
 "click":{"sent":true,"via":"action","logged":true,"pixel_ok":true}}
```

Exit code 0 only when both streams produced frames and the click was logged.

## Benchmark JSONL (`CUA_BENCH_JSONL`)

When the harness drives an example as a language lane it sets
`CUA_BENCH_JSONL=<path>` plus `CUA_BENCH_TARGET` (`display:<id>` or
`window:<title>`), `CUA_BENCH_SECONDS`, and `CUA_BENCH_AUDIO=0|1`. The example
then skips steps 2, 5 and 6, streams only that target, and appends one JSON
object per line:

```json
{"t":"open","unix_ns":1790000000000000000}
{"t":"frame","unix_ns":…,"seq":41,"bytes":5321,"key":false,"cap_us":123456789,"w":1024,"h":640,"tc_ms":1790000000123}
{"t":"audio","unix_ns":…,"pts_us":123450000,"bytes":83,"samples":960}
{"t":"end","unix_ns":…,"cpu_user_s":1.23,"cpu_sys_s":0.20}
```

- `unix_ns` is the client's wall clock when the item was *delivered to the
  example's code* (after decode for decoded callbacks).
- `tc_ms` is optional (null when not decoded): the timecode read from the
  frame, see below. It is what glass-to-glass latency is computed from.
- `cpu_*` come from `getrusage(RUSAGE_SELF)` (or the language's equivalent).

## Bench timecode (for `tc_ms`)

The benchmark fixture (`libs/cua/bench/streaming/fixtures/benchfix.py`) draws a
strip of 48 cells, 16×16 px each, at the top-left of its window content
(window-target frames: frame pixel (0, 0)). Cell `i` is white (1) or black
(0); read it as `mean(R,G,B) > 128` over the 4×4 pixels at the cell centre
(`x = 16 i + 6 … 9`, `y = 6 … 9`, times the frame scale).

| cells | content |
|---|---|
| 0-3 | sync `1 0 1 0` |
| 4-35 | guest wall clock, unix ms mod 2³², MSB first |
| 36-43 | XOR of the four value bytes |
| 44-47 | sync `0 1 0 1` |

Discard the frame's `tc_ms` unless both syncs and the checksum match. Rebuild
the full ms value from the client's own clock: take the 2³² window nearest to
`unix_ns / 10⁶`.
