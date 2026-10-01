# Streaming example: TypeScript in the browser

The shared scenario ([`../SCENARIO.md`](../SCENARIO.md)) in a web page. The
page is plain ES modules, so there is no bundler. It uses:

- `@trycua/cua/browser` (the wasm build of `cua-sdk`, over gRPC-Web) for the
  env calls: `health`, `sh`, `click`, and `callJson` for
  `StreamService.ListTargets`, `OpenMedia` and `CloseMedia`;
- the media plane ([`libs/cua/proto/MEDIA.md`](../../../libs/cua/proto/MEDIA.md))
  on a browser `WebSocket` to `/media`. The ticket goes in the subprotocol
  `rcdp.v2, cua.ticket.<ticket>` and never in the URL. Binary video
  packets are length-prefixed JSON headers, and audio packets are `RAU2`;
- WebCodecs `VideoDecoder` for H.264. The input is Annex B with no
  `description`, and the codec string comes from the SPS. Frames are drawn
  onto a `<canvas>`;
- WebCodecs `AudioDecoder` for Opus, played with `AudioContext` (with a 40 ms
  scheduling buffer) and written to WAV;
- the click as a media-plane `action` with a pixel basis on a session opened
  with `SESSION_POLICY_ALLOW_ACTIVATION`. The fallback is `env.click` in
  screen coordinates.

The browser SDK has no media-session API, so the page opens the WebSocket
itself. That is the only part of the media plane it implements.

## Build (once)

Run these from the repository root:

```sh
(cd libs/cua/typescript && npm ci && npm run build:browser)   # wasm32 + wasm-bindgen 0.2.126 -> typescript/browser/
cd examples/streaming/typescript-web
npm install                  # links @trycua/cua (file:) and installs playwright
npm run install-browser      # chromium-headless-shell into node_modules (PLAYWRIGHT_BROWSERS_PATH=0)
```

`serve.mjs` serves this directory at `/`, the SDK's `browser/` at `/sdk/`,
and `@ubjs/core` (the generated bindings' only bare import) at `/ubjs-core/`
through the import map in `index.html`.

## Interactive

```sh
S=libs/cua/bench/streaming/scripts/space.sh
$S start && eval "$($S env)"
node examples/streaming/typescript-web/serve.mjs 8765     # http://127.0.0.1:8765/
```

Open the page, paste `CUA_ENV_URL` and `CUA_ENV_TOKEN`, and press **Run
scenario**. The page draws the video, plays the audio, and offers the two WAV
files as downloads. The spacesd serves gRPC-Web with CORS on its own port,
so a page on another loopback origin can reach it.

## Headless (Playwright)

```sh
cd examples/streaming/typescript-web
CUA_OUT_DIR=./out node headless.mjs
```

`headless.mjs` serves the page on an ephemeral loopback port. It launches
Playwright's headless Chromium with `chromium.launch()`, which gives a fresh
temporary profile that is removed on close. It never uses an installed
Chrome or its profile. It injects the configuration through
`addInitScript` (so the token stays out of URLs) and relays what the page
reports through `exposeFunction("cuaReport")`: the log, the `SUMMARY` line,
the WAV files (written to `$CUA_OUT_DIR`) and the bench JSONL.

## Bench mode

```sh
$S fixture timecode
CUA_BENCH_JSONL=/tmp/web.jsonl CUA_BENCH_TARGET='window:CUA Bench Timecode' \
CUA_BENCH_SECONDS=5 CUA_BENCH_AUDIO=1 libs/cua/bench/streaming/shims/browser.sh
```

- `frame` lines carry the packet's `seq`, `bytes`, `key` and `cap_us`.
  `unix_ns` is the time of the `VideoDecoder` output callback, and `tc_ms` is
  read from the canvas (`getImageData` of the timecode strip) right after
  `drawImage`.
- `audio` lines are written at the `AudioDecoder` output. `bytes` is the size
  of the Opus packet.
- The browser has no `getrusage`. The runner writes the `end` line with the
  CPU time of every Chromium process (browser, renderer, GPU, utility), taken
  from CDP `SystemInfo.getProcessInfo`. That figure is user and system time
  combined, so it goes in `cpu_user_s`, and `cpu_sys_s` is `0`
  (`cpu_scope` says so).
- `browser.sh` exits 3 with `skipped: …` when Playwright, its Chromium or the
  SDK browser build is missing. It exits 2 when the harness environment is
  missing.

## Notes

- `last_hash` is FNV-1a 64 over BGRA read back from the canvas. The canvas
  applies the browser's YUV-to-sRGB conversion, so the hash is not comparable
  with the Node/Rust hashes, which are taken from the SDK decoder's BGRA.
- Like the Node example, the click is not observed on the current
  `linux` image. The action is delivered (`action_delivered:
  true`), but the grid fixture logs no `button_press`. See the Node README.
