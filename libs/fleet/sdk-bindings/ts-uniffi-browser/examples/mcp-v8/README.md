# Running the browser/WASM SDK inside mcp-v8

This example embeds the Cyclops browser/WASM SDK in
[mcp-v8](https://github.com/r33drichards/mcp-js) — a Rust MCP server that runs
JavaScript in a sandboxed V8 isolate (`deno_core`, neither Node nor a
browser). An agent driving mcp-v8's `run_js` tool can then orchestrate fleet
resources (templates, pools, claims) by writing JavaScript against the same
generated builder surface the browser uses.

Verified end to end on 2026-08-18 against stock mcp-v8 **v0.18.2**:
HTTP module import, WASM compile + uniffi checksum init, the builder
contract, and an authenticated `listPools` round trip through mcp-v8's
policy-gated `fetch`, with the isolate draining cleanly afterwards.

This is a diagnostic example, not a certified harness: the canonical
browser/WASM gates remain the Playwright specs in `../../tests/`.

## Why this works

- mcp-v8 executes code as an ES module in bare V8 with `fetch`
  (OPA/Rego-gated), `setTimeout`, TextEncoder/TextDecoder, and native
  `WebAssembly`. The SDK's wasm is instantiated from bytes fetched at run
  time; the transport is `web_sys`-over-`fetch`
  (`connect_browser_with_access_token`), so no Node N-API is involved —
  exactly the surface the browser build already targets.
- What bare V8 lacks is filled by a ~100-line JS prelude in
  [`run-js-payload.js`](./run-js-payload.js):

  | Gap | Shim |
  |---|---|
  | `window` global (`web_sys::window()` does `instanceof Window`) | `Window` class with `Symbol.hasInstance` matching `globalThis` |
  | `Headers` / `Request` / `Response` globals (absent in v0.18.2; present on newer mcp-js `main`) | minimal Request/Headers classes + a fetch wrapper translating `fetch(Request)` → `fetch(url, init)` |
  | cleared long timers wedge the event loop (`@ubjs/core` arms a ~25-day keep-alive `setTimeout` around every async Rust call; v0.18.2's `clearTimeout` leaves the sleep op pending) | clamp: never arm timers > 60 s. Fixed upstream in mcp-js by unref'ing cleared timers |

## Reproduce

1. Build the WASM bundle (needs the `wasm-bindgen` CLI matching the pinned
   `wasm-bindgen` crate version, plus the repo's usual prerequisites):

   ```sh
   npm ci
   npm run build:wasm
   npx esbuild examples/mcp-v8/entry.ts --bundle --format=esm \
     --platform=browser --outfile=dist-mcp-v8/fleet-sdk-web.js
   cp ts/wasm-bindgen/index_bg.wasm dist-mcp-v8/
   ```

   [`entry.ts`](./entry.ts) mirrors the generated `ts/index.web.ts` but takes
   the wasm location as an argument to `uniffiInitAsync` instead of a bundler
   asset import, so the embedding runtime controls how the bytes are fetched.

2. Serve the bundle and a mock Cyclops backend (or point `API_BASE` at a real
   deployment):

   ```sh
   node examples/mcp-v8/mock-cyclops.mjs dist-mcp-v8 8788
   ```

3. Run mcp-v8 with external modules enabled, a loopback fetch policy, and a
   raised heap cap (the wasm linear memory counts against it; the 8 MB
   default is far too small):

   ```sh
   mcp-v8 --http-port 8790 --allow-external-modules --heap-memory-max 512 \
     --policies-json '{"fetch":{"policies":[{"url":"file:///abs/path/to/examples/mcp-v8/fetch.rego"}]}}'
   ```

4. Submit the payload and read the output:

   ```sh
   curl -s -X POST 'http://127.0.0.1:8790/api/exec?execution_timeout_secs=60&heap_memory_max_mb=512' \
     -H 'Content-Type: application/javascript' \
     --data-binary @examples/mcp-v8/run-js-payload.js
   curl -s http://127.0.0.1:8790/api/executions/<id>/output
   ```

   Expected output ends with `listPools ok: [...demo-pool...]`.

## Limitations

- **Stateless only.** mcp-v8 heap snapshots require a V8 `SnapshotCreator`
  isolate, which disables WebAssembly — heap persistence and this SDK are
  mutually exclusive. Re-import and re-init per execution (~1.7 MB wasm,
  sub-second), or keep state on the Cyclops side.
- **Timer wedge on v0.18.2.** Without the prelude's timer clamp, every async
  SDK call leaves the isolate pending for ~25 days and permanently consumes
  one of the server's execution slots (`min(concurrency)` = CPU count) —
  the execution timeout cannot reclaim it. Keep the clamp until running a
  server with the upstream fix.
- The bundle is not published anywhere; mcp-v8 imports it over HTTP from a
  host allowed by its module/fetch policies (loopback in this example).
