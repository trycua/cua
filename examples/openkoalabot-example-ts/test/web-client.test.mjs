// The web UI's streaming path, run in Node against cua-test-fixtures'
// MockServer env (scripted `/media` socket): the wasm `@trycua/cua/browser`
// client opens the session over gRPC-Web (`fetch`), and the ticketed
// WebSocket runs the same parser the page runs. Skipped without the
// fixtures binary or the browser build.
import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { existsSync } from "node:fs"
import { createInterface } from "node:readline"
import { fileURLToPath } from "node:url"
import { test } from "node:test"
import { WebSocket as WsClient } from "ws"

const fixtures = process.env.CUA_TEST_FIXTURES ?? fileURLToPath(new URL("../../../libs/cua/target/debug/cua-test-fixtures", import.meta.url))
const browserBuilt = existsSync(fileURLToPath(new URL("../../../libs/cua/typescript/browser/index.js", import.meta.url)))

test(
  "wasm gRPC-Web OpenMedia + ticketed /media socket",
  { skip: (!existsSync(fixtures) && "cua-test-fixtures is not built") || (!browserBuilt && "npm run build:browser in libs/cua/typescript"), timeout: 60_000 },
  async () => {
    const { loadBrowserSdk } = await import("../dist/core/runtime.js")
    const { attachMedia, closeMedia, openDesktopMedia } = await import("../dist/core/webmedia.js")
    const child = spawn(fixtures, [], { stdio: ["pipe", "pipe", "ignore"] })
    try {
      const fx = JSON.parse(await new Promise((r) => createInterface({ input: child.stdout }).once("line", r)))
      const sdk = await loadBrowserSdk()
      const env = await sdk.Cua.embedded(sdk.CuaConfig.create({})).spacesd(fx.env_url, fx.env_token)
      const media = await openDesktopMedia(env, fx.env_url, { maxFps: 5, maxDimension: 640 })
      assert.match(media.wsUrl, /^ws:\/\/127\.0\.0\.1:\d+\/media\?ticket=/)
      // Two fixture limits shape this: its `/media` sniffing is per TCP
      // connection, so Node's global WebSocket (undici), which reuses the
      // gRPC-Web fetch's keep-alive connection for the upgrade, reaches the
      // gRPC server instead; and it accepts the ticket subprotocol without
      // echoing it, which a WHATWG client must reject. The `ws` client (always
      // a fresh connection) with the query ticket drives the same parser.
      const { ws, state, ticketVia } = await attachMedia(WsClient, media, 20_000)
      ws.close(1000)
      assert.ok(["subprotocol", "query"].includes(ticketVia))
      assert.equal(state.handshakeOk, true, "hello then session_opened")
      assert.equal(state.firstFrameKeyframe, true, "the first packet on a new socket is a keyframe")
      await closeMedia(env, media.mediaSessionId)
    } finally {
      child.stdin.end()
    }
  },
)

test(
  "the scenario's web-client probe over the wasm client",
  { skip: (!existsSync(fixtures) && "cua-test-fixtures is not built") || (!browserBuilt && "npm run build:browser in libs/cua/typescript"), timeout: 60_000 },
  async () => {
    const { webClientProbe } = await import("../dist/core/runtime.js")
    const child = spawn(fixtures, [], { stdio: ["pipe", "pipe", "ignore"] })
    try {
      const fx = JSON.parse(await new Promise((r) => createInterface({ input: child.stdout }).once("line", r)))
      const r = await webClientProbe(null, { maxFps: 5, maxDimension: 640, timeoutMs: 10_000, url: fx.env_url, token: fx.env_token })
      assert.equal(r.via, "browser-wasm")
      assert.equal(r.state.handshakeOk, true)
      assert.equal(r.state.firstFrameKeyframe, true)
      assert.equal(r.state.firstFrameAnnexBKeyframe, true, "the fixture keyframe carries SPS+PPS+IDR")
    } finally {
      child.stdin.end()
    }
  },
)
