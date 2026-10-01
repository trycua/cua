# Browser live lifecycle example

This development-only page runs the same pool, claim, sandbox, service-request,
and cleanup lifecycle as the Node example. It uses browser `fetch` for the
UniFFI `HttpClient` callback.

## Security

The browser never receives OAuth client credentials. A trusted runner mints a
short-lived access token through client credentials, then injects only that
token through `window.__CYCLOPS_BROWSER_CONFIG__` before the page starts.

A browser app hosted on a different origin needs the control plane to allow
that origin with CORS. Do not work around a CORS failure by exposing client
credentials to browser code.

The GitHub Actions live test keeps its control-plane calls same-origin: Playwright
intercepts the generated bundle at a temporary `https://run.cua.ai` path. Vite on
`http://127.0.0.1:4174` remains only for the local artifact and UI-boundary tests.

## Build prerequisite

The generated browser sources import `ts/wasm-bindgen/index.js`. That module is
emitted by the UniFFI React Native browser/WASM pipeline and is not checked into
this binding directory. `ts/wasm-bindgen/`, its Rust build directory, and the
bundled `examples/dist/` output are generated artifacts and are not committed.

Build the artifacts from the repository root:

```bash
cyclops-cs/scripts/build-browser-sdk-binding.sh
```

This also applies the pinned compatibility patch required by the UBRN WASM
template and bundles `browser.ts` as `examples/dist/browser.js`.

## Serve and run

Set `window.__CYCLOPS_BROWSER_CONFIG__` before loading the page, then serve this
directory and select **Run Lifecycle**. The example creates a one-replica pool
exposing `mcp` on port `3000`, sends `GET /health`, and cleans up its resources.

GitHub Actions uses the repository runner script, which mints and masks the
token before it runs the browser artifact, browser boundary, and live lifecycle
tests:

```bash
cyclops-cs/scripts/run-browser-sdk-binding.sh
```
