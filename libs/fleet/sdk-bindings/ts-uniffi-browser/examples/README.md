# Browser live lifecycle example

This development-only page runs the same pool, claim, sandbox, service-request,
and cleanup lifecycle as the Node example. It uses browser `fetch` for the
UniFFI `HttpClient` callback.

## Security

The page accepts an OAuth client secret and the generated SDK passes it to the
token endpoint. Do not deploy this page with a production secret. Use a
development-only OAuth client, HTTPS, and a trusted origin with required CORS
policies for the control plane and token endpoint.

## Build prerequisite

The generated browser sources import `ts/wasm-bindgen/index.js`. That module is
emitted by the UniFFI React Native browser/WASM pipeline and is not checked into
this binding directory. Provide a project `ubrn.config.yaml` and run that WASM
build pipeline first so its crate and JS glue are generated beside `ts`.

After the WASM output exists, bundle `browser.ts` as an ESM browser entrypoint
to `examples/dist/browser.js` using the project's browser bundler. `index.html`
loads that compiled bundle rather than raw TypeScript.

## Serve and run

Serve this directory after bundling, for example with `npx serve .`, then open
the printed URL. Enter the settings, development OAuth client, image, and
namespace, then select **Run Lifecycle**. The example creates a one-replica pool
exposing `mcp` on port `3000`, sends `GET /health`, and cleans up its resources.
