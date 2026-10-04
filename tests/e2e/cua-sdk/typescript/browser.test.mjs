// create-pool-browser: @trycua/cua/browser (the wasm32 build) talks
// gRPC-Web to a spacesd.
//
// * wasm in Node: the browser module imported directly (fetch transport).
// * Chromium (Playwright): the same flow in a real page, served with a
//   same-origin proxy to the env fixture (no CORS assumptions). Runs when
//   `playwright` resolves and a Chromium is installed (CI installs it).
import assert from "node:assert/strict"
import { existsSync, readFileSync } from "node:fs"
import { createServer, request as httpRequest } from "node:http"
import { createRequire } from "node:module"
import { extname, join } from "node:path"
import { after, before } from "node:test"

import * as e from "./lib.mjs"

const BROWSER = process.env.CUA_TS_BROWSER ?? join(e.CUA_ROOT, "typescript", "browser")
const built = existsSync(join(BROWSER, "cua_sdk.js"))
const needBuild = () => { if (!built) e.skip("browser build missing (cd libs/cua/typescript && npm run build:browser)") }

let fx
before(async () => {
  if (!e.laneEnabled("hermetic")) fx = await e.startFixtures()
})
after(async () => fx?.stop())

// The flow both hosts run (kept as a string so the page runs the same code).
// The wasm transport uses globalThis.fetch, so Node runs it too.
const FLOW = `async (sdk, cfg) => {
  const c = sdk.Cua.embedded(sdk.CuaConfig.create({}))
  const env = await c.spacesd(cfg.env, cfg.envToken)
  const out = await env.sh("echo from-browser", undefined)
  let unauth = false
  try { await c.spacesd(cfg.env, "wrong") } catch (err) { unauth = sdk.CuaError.Unauthenticated.instanceOf(err) }
  return { stdout: new TextDecoder().decode(out.stdout), unauth }
}`

function check(r) {
  assert.equal(r.stdout, "from-browser\n")
  assert.equal(r.unauth, true)
}

e.e2eTest("create-pool-browser", "hermetic", "wasm build in Node: env over gRPC-Web", async () => {
  needBuild()
  const sdk = await import(join(BROWSER, "index.js"))
  await sdk.initialize(readFileSync(join(BROWSER, "wasm-bindgen", "index_bg.wasm")))
  assert.equal(typeof globalThis.window, "undefined", "Node has no window")
  const flow = eval(FLOW)
  const r = await flow(sdk, { env: fx.env_url, envToken: fx.env_token })
  check(r)
})

const MIME = { ".js": "text/javascript", ".mjs": "text/javascript", ".wasm": "application/wasm", ".html": "text/html" }

/** Static files + a same-origin proxy: /env/* -> env fixture. */
function serve() {
  const ubjs = join(e.CUA_ROOT, "typescript", "node_modules", "@ubjs", "core", "dist", "esm")
  const page = `<!doctype html><script type="importmap">{"imports":{"@ubjs/core":"/ubjs/index.js"}}</script>`
  const proxy = (target, req, res, strip) => {
    const u = new URL(target)
    const up = httpRequest({ host: u.hostname, port: u.port, method: req.method, path: req.url.slice(strip.length) || "/",
      headers: { ...req.headers, host: u.host } }, (r) => { res.writeHead(r.statusCode, r.headers); r.pipe(res) })
    up.on("error", (err) => { res.writeHead(502); res.end(String(err)) })
    req.pipe(up)
  }
  const server = createServer((req, res) => {
    if (req.url.startsWith("/env/")) return proxy(fx.env_url, req, res, "/env")
    let file
    if (req.url === "/") { res.writeHead(200, { "content-type": "text/html" }); return res.end(page) }
    if (req.url.startsWith("/ubjs/")) file = join(ubjs, req.url.slice(6))
    else if (req.url.startsWith("/browser/")) file = join(BROWSER, req.url.slice(9))
    if (!file || !existsSync(file)) { res.writeHead(404); return res.end() }
    res.writeHead(200, { "content-type": MIME[extname(file)] ?? "application/octet-stream" })
    res.end(readFileSync(file))
  })
  return new Promise((r) => server.listen(0, "127.0.0.1", () => r(server)))
}

e.e2eTest("create-pool-browser", "hermetic", "Chromium (Playwright): env over gRPC-Web", async () => {
  needBuild()
  let chromium
  try {
    const from = process.env.CUA_E2E_PLAYWRIGHT_DIR ? join(process.env.CUA_E2E_PLAYWRIGHT_DIR, "package.json") : import.meta.url
    ;({ chromium } = createRequire(from)("playwright"))
  } catch {
    e.skip("playwright is not installed (npm i -D playwright && npx playwright install chromium)")
  }
  const server = await serve()
  const origin = `http://127.0.0.1:${server.address().port}`
  let browser
  try {
    browser = await chromium.launch()
  } catch (err) {
    server.close()
    e.skip(`no Chromium for Playwright: ${String(err.message).split("\n")[0]}`)
  }
  try {
    const page = await browser.newPage()
    await page.goto(origin)
    const r = await page.evaluate(async ([flowSrc, cfg]) => {
      const sdk = await import("/browser/index.js")
      await sdk.initialize("/browser/wasm-bindgen/index_bg.wasm")
      return await (0, eval)(flowSrc)(sdk, cfg)
    }, [FLOW, { env: `${origin}/env`, envToken: fx.env_token }])
    check(r)
  } finally {
    await browser.close()
    server.close()
  }
}, { timeout: 300_000 })
