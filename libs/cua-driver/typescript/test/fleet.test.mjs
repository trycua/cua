import assert from "node:assert/strict"
import { existsSync } from "node:fs"
import { test } from "node:test"
import { fileURLToPath } from "node:url"

const triple = process.platform === "darwin"
  ? `darwin-${process.arch}`
  : process.platform === "win32"
    ? `win32-${process.arch}-msvc`
    : `linux-${process.arch}-${process.report.getReport().header.glibcVersionRuntime ? "gnu" : "musl"}`
const library = process.platform === "darwin" ? "libcua_driver_sdk.dylib"
  : process.platform === "win32" ? "cua_driver_sdk.dll" : "libcua_driver_sdk.so"
const nativeAvailable = existsSync(fileURLToPath(new URL(
  `../node_modules/@trycua/cua-driver-${triple}/${library}`, import.meta.url,
)))
const encoder = new TextEncoder()
const decoder = new TextDecoder()
const bytes = value => encoder.encode(value).buffer

function fixture({ sse = false } = {}) {
  const sandbox = { namespace: "tenant", claim: "claim", name: "guest", services: ["mcp", "server"] }
  const calls = []
  const sessions = new Set()
  let sequence = 0
  const client = {
    async serviceRequest(target, service, path, request, options) {
      assert.deepEqual(target, { namespace: "tenant", claim: "claim", name: "guest", services: ["mcp", "server"] })
      assert.equal(service, "mcp")
      assert.equal(path, "/mcp")
      assert.equal(request.url, "https://service.invalid/mcp")
      assert.equal(typeof request.timeoutSecs, "bigint")
      assert.ok(options.signal instanceof AbortSignal)
      assert.ok(request.body instanceof ArrayBuffer)
      const rpc = request.body.byteLength ? JSON.parse(decoder.decode(request.body)) : null
      const headers = Object.fromEntries(request.headers.map(({ name, value }) => [name.toLowerCase(), value]))
      calls.push({ target, service, path, request, rpc })
      if (client.intercept) await client.intercept(rpc, options.signal)
      if (request.method === "DELETE") {
        sessions.delete(headers["mcp-session-id"])
        return { status: 200, headers: [], body: new ArrayBuffer(0) }
      }
      let result
      const responseHeaders = []
      if (rpc.method === "initialize") {
        const session = `http-${++sequence}`
        sessions.add(session)
        responseHeaders.push({ name: "Mcp-Session-Id", value: session })
        result = { protocolVersion: "2025-06-18", capabilities: { experimental: { "ai.cua.driver.envelopes": { version: 1 } } } }
      } else {
        const session = headers["mcp-session-id"]
        assert.ok(sessions.has(session))
        if (rpc.method === "notifications/initialized") {
          return { status: 202, headers: [], body: new ArrayBuffer(0) }
        }
        if (rpc.method === "cua/driver/v1/open") {
          result = { connection_id: session, generation: `gen-${session}`, public_session: "guest-label", capabilities: {
            minimum_envelope_version: 1, maximum_envelope_version: 1, supports_cancellation: true,
          } }
        } else if (rpc.method === "cua/driver/v1/exchange") {
          result = { envelope_version: 1, request_id: rpc.params.envelope.request_id, ok: true,
            result: { content: [{ type: "text", text: "typed-fleet-fixture" }], isError: false }, completion_known: true }
        } else {
          assert.ok(["cua/driver/v1/close", "cua/driver/v1/cancel"].includes(rpc.method))
          result = { ok: true }
        }
      }
      const json = JSON.stringify({ jsonrpc: "2.0", id: rpc.id, result })
      responseHeaders.push({ name: "Content-Type", value: sse ? "text/event-stream" : "application/json" })
      return { status: 200, headers: responseHeaders, body: bytes(sse ? `event: message\r\ndata: ${json}\r\n\r\n` : json) }
    },
  }
  return { client, sandbox, calls, sessions }
}

for (const sse of [false, true]) {
  test(`Fleet returns the actual typed Rust Driver (${sse ? "SSE" : "JSON"})`, { skip: !nativeAvailable }, async () => {
    const { CuaDriver } = await import("@trycua/cua-driver")
    const { connectFleetDriver } = await import("@trycua/cua-driver/fleet")
    const f = fixture({ sse })
    const connection = await connectFleetDriver(f)
    try {
      assert.ok(connection.driver instanceof CuaDriver)
      f.sandbox.name = "different-guest"
      f.sandbox.services.length = 0
      const result = await connection.driver.getScreenSize({})
      assert.equal(result.text, "typed-fleet-fixture")
      assert.equal(f.calls.at(-1).rpc.params.envelope.name, "get_screen_size")
      assert.ok(connection.sessionName)
    } finally {
      const closing = connection.close()
      assert.equal(connection.close(), closing)
      await closing
    }
    assert.equal(f.sessions.size, 0)
    const count = f.calls.length
    await assert.rejects(connection.driver.getScreenSize({}))
    assert.equal(f.calls.length, count)
  })
}

test("Fleet rejects unadvertised service and pre-aborted connections before requests", { skip: !nativeAvailable }, async () => {
  const { connectFleetDriver } = await import("@trycua/cua-driver/fleet")
  const f = fixture()
  await assert.rejects(connectFleetDriver({ ...f, service: "missing" }), /does not advertise/)
  await assert.rejects(connectFleetDriver({ ...f, signal: AbortSignal.abort() }), /aborted/)
  assert.equal(f.calls.length, 0)
})

test("Fleet callback errors exclude sensitive diagnostics", { skip: !nativeAvailable }, async () => {
  const { connectFleetDriver } = await import("@trycua/cua-driver/fleet")
  const f = fixture()
  f.client.intercept = () => { throw new Error("secret-token-private-response") }
  await assert.rejects(connectFleetDriver(f), error => {
    assert.doesNotMatch(String(error), /secret-token-private-response/)
    assert.doesNotMatch(JSON.stringify(error), /secret-token-private-response/)
    return true
  })
})

test("aborting a Fleet connection lets Rust cancel and drain before closing", { skip: !nativeAvailable, timeout: 10_000 }, async () => {
  const { connectFleetDriver } = await import("@trycua/cua-driver/fleet")
  const f = fixture()
  const controller = new AbortController()
  const connection = await connectFleetDriver({ ...f, signal: controller.signal })
  let started
  const pending = new Promise(resolve => { started = resolve })
  let callbackSignal
  let release
  const delayed = new Promise(resolve => { release = resolve })
  f.client.intercept = (rpc, signal) => {
    if (rpc?.method === "cua/driver/v1/exchange") {
      callbackSignal = signal
      started()
      return delayed
    }
    if (rpc?.method === "cua/driver/v1/cancel") release()
  }
  const action = connection.driver.getScreenSize({})
  const rejected = assert.rejects(action)
  await pending
  controller.abort()
  await connection.close()
  await rejected
  assert.equal(callbackSignal.aborted, false)
  assert.equal(f.sessions.size, 0)
})

test("aborting during receiver open cannot return a live late Driver", { skip: !nativeAvailable, timeout: 10_000 }, async () => {
  const { connectFleetDriver } = await import("@trycua/cua-driver/fleet")
  const f = fixture()
  const controller = new AbortController()
  let started
  const pending = new Promise(resolve => { started = resolve })
  let release
  const delayed = new Promise(resolve => { release = resolve })
  f.client.intercept = rpc => {
    if (rpc?.method === "cua/driver/v1/open") { started(); return delayed }
  }
  const opening = connectFleetDriver({ ...f, signal: controller.signal })
  const rejected = assert.rejects(opening)
  await pending
  controller.abort()
  release()
  await rejected
  assert.equal(f.sessions.size, 0)
})
