// Smoke test of the browser (wasm32) build, run in Node: the wasm module
// talks gRPC-Web to cua-test-fixtures through `fetch`, exactly as it does in
// a page. Skipped until `npm run build:browser` has produced browser/.
import assert from "node:assert/strict"
import { existsSync, readFileSync } from "node:fs"
import { join } from "node:path"
import { after, before, test } from "node:test"
import { fileURLToPath } from "node:url"

import { binary, startFixtures } from "./fixtures.mjs"

const browser = fileURLToPath(new URL("../browser/", import.meta.url))
const built = existsSync(join(browser, "cua_sdk.js"))

let fx
let sdk
before(async () => {
  if (!built || !binary("CUA_TEST_FIXTURES", "cua-test-fixtures")) return
  fx = await startFixtures()
  sdk = await import(join(browser, "index.js"))
  await sdk.initialize(readFileSync(join(browser, "wasm-bindgen", "index_bg.wasm")))
})
after(async () => {
  await fx?.stop()
})

test(
  "browser build: env over gRPC-Web and typed errors",
  { skip: !built ? "browser build missing (npm run build:browser)" : !binary("CUA_TEST_FIXTURES", "cua-test-fixtures") },
  async () => {
    const cua = sdk.Cua.embedded(sdk.CuaConfig.create({}))
    const env = await cua.spacesd(fx.env_url, fx.env_token)
    const caps = await env.capabilities()
    assert.ok(caps.version.length > 0)
    const out = await env.sh("echo from-wasm", undefined)
    assert.equal(new TextDecoder().decode(out.stdout), "from-wasm\n")
    assert.equal(out.exit.code, 0)
    await env.setClipboard("wasm clip")
    assert.equal(await env.getClipboard(), "wasm clip")
    assert.match(await env.callJson("SystemService/Health", "{}"), /^\{/)
    await assert.rejects(cua.spacesd(fx.env_url, "wrong"), (e) =>
      sdk.CuaError.Unauthenticated.instanceOf(e),
    )
    assert.throws(() => cua.fleet(), (e) => sdk.CuaError.ProviderNotConfigured.instanceOf(e))
  },
)
