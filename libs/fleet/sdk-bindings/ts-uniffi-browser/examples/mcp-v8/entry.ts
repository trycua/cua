// Entry point for embedding the browser/WASM SDK in a non-browser JS runtime
// (mcp-v8 / deno_core). Unlike the generated ts/index.web.ts, the WASM binary
// location is not baked in as a bundler asset import: the host passes a URL or
// BufferSource to uniffiInitAsync, because the embedding runtime decides how
// the .wasm bytes are fetched (e.g. through a policy-gated fetch).
//
// Bundle (after `npm run build:wasm`):
//   npx esbuild examples/mcp-v8/entry.ts --bundle --format=esm \
//     --platform=browser --outfile=dist-mcp-v8/fleet-sdk-web.js
//   cp ts/wasm-bindgen/index_bg.wasm dist-mcp-v8/
export * from "../../ts/cyclops_sdk_schema"
export * from "../../ts/fleet_sdk"

import * as cyclops_sdk_schema from "../../ts/cyclops_sdk_schema"
import * as fleet_sdk from "../../ts/fleet_sdk"

import initAsync from "../../ts/wasm-bindgen/index.js"

export async function uniffiInitAsync(
  moduleOrPath: string | URL | BufferSource | WebAssembly.Module,
): Promise<void> {
  await initAsync({ module_or_path: moduleOrPath })
  cyclops_sdk_schema.default.initialize()
  fleet_sdk.default.initialize()
}

export default {
  cyclops_sdk_schema,
  fleet_sdk,
}
