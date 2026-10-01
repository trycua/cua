// `@trycua/cua/spaces` pieces that need no native library and no Space:
// the placement gate, the turn lock, the transcript adapter, error mapping,
// Thread over a scripted Space, the MCP session/transports against loopback
// fakes, and the host presentation adapter against a stand-in control server.
// Ported from @trycua/spaces' unit and host suites (the /cmd, base64 and
// shell-harness tests are gone with that wire; cua-spaces' Rust tests own it).
import assert from "node:assert/strict"
import { createServer } from "node:http"
import { mkdtempSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import { isSpacesError, toSpacesError, SpacesError } from "../dist/spaces/errors.js"
import { TranscriptAdapter, detectApproval } from "../dist/spaces/events.js"
import { SpaceTurnLock, Thread, validatePlacement, startThread } from "../dist/spaces/thread.js"
import { HostPresentation, cuaHome, readControlEndpoint } from "../dist/spaces/host.js"
import {
  McpHttpTransport,
  McpSession,
  TauriTransport,
  ToolError,
  TransportError,
} from "../dist/spaces/transport/index.js"

// -- placement / isolation --------------------------------------------------

test("placement cannot be defaulted into", () => {
  assert.throws(() => validatePlacement(undefined), (e) => isSpacesError(e, "usage"))
})

test("shared placement without the acknowledgement is refused with code isolation", () => {
  assert.throws(
    () => validatePlacement({ type: "shared", space: "space://direct/h:1" }),
    (error) => isSpacesError(error, "isolation") && /not a security boundary/.test(error.message),
  )
})

test("a falsy-but-present acknowledgement is still refused", () => {
  for (const value of [false, "true", 1, null]) {
    assert.throws(
      () => validatePlacement({ type: "shared", space: "s", acknowledgeNoIsolation: value }),
      (e) => isSpacesError(e, "isolation"),
      `accepted ${JSON.stringify(value)}`,
    )
  }
})

test("an acknowledged shared placement and a dedicated placement both pass", () => {
  assert.equal(validatePlacement({ type: "dedicated" }).type, "dedicated")
  assert.equal(validatePlacement({ type: "shared", space: "s", acknowledgeNoIsolation: true }).type, "shared")
})

// -- the per-Space turn lock ------------------------------------------------

test("the turn lock serializes turns in call order", async () => {
  const lock = new SpaceTurnLock()
  const order = []
  const first = lock.enqueue("turn-1")
  const second = lock.enqueue("turn-2")
  assert.equal(first.waitingBehind, null)
  const releaseFirst = await first.acquired
  order.push("first")
  let secondAcquired = false
  const secondPromise = second.acquired.then((release) => {
    secondAcquired = true
    order.push("second")
    return release
  })
  await new Promise((resolve) => setTimeout(resolve, 10))
  assert.equal(secondAcquired, false, "second turn must not run while the first holds the Space")
  releaseFirst()
  ;(await secondPromise)()
  assert.deepEqual(order, ["first", "second"])
})

test("a failed turn does not wedge the Space forever", async () => {
  const lock = new SpaceTurnLock()
  const first = lock.enqueue("turn-1")
  const second = lock.enqueue("turn-2")
  ;(await first.acquired)()
  const releaseSecond = await Promise.race([
    second.acquired,
    new Promise((_, reject) => setTimeout(() => reject(new Error("wedged")), 500)),
  ])
  releaseSecond()
})

// -- transcript adapter -----------------------------------------------------

test("the adapter emits only what is new between polls", () => {
  const adapter = new TranscriptAdapter()
  assert.equal(adapter.ingest("line one\n").filter((e) => e.kind === "text").length, 1)
  assert.deepEqual(adapter.ingest("line one\n"), [])
  const texts = adapter.ingest("line one\nline two\n").filter((e) => e.kind === "text")
  assert.equal(texts.length, 1)
  assert.equal(texts[0].text, "line two")
})

test("every derived event says it is derived", () => {
  const events = new TranscriptAdapter().ingest("wrote /root/out/report.md see https://example.com/x\n")
  assert.ok(events.length > 0)
  for (const event of events) assert.equal(event.derived, true, `${event.kind} was not marked derived`)
})

test("the adapter reports lost output instead of dropping it silently", () => {
  const adapter = new TranscriptAdapter()
  adapter.ingest("aaaa\n")
  const errors = adapter.ingest("zzzz\n").filter((e) => e.kind === "error")
  assert.equal(errors.length, 1)
  assert.match(errors[0].message, /scrolled past/)
})

test("a scrolled window with overlap emits only the unseen remainder", () => {
  const adapter = new TranscriptAdapter()
  adapter.ingest("one\ntwo\n")
  const events = adapter.ingest("two\nthree\n")
  assert.deepEqual(events.filter((e) => e.kind === "text").map((e) => e.text), ["three"])
  assert.equal(events.filter((e) => e.kind === "error").length, 0)
})

test("files and images are separated, links are captured, directories are not", () => {
  const events = new TranscriptAdapter().ingest(
    "saved /root/work/chart.png and /root/work/notes.md in /root/work — see https://example.com/a).\n",
  )
  assert.deepEqual(events.filter((e) => e.kind === "file").map((e) => e.path), ["/root/work/notes.md"])
  const images = events.filter((e) => e.kind === "image")
  assert.equal(images.length, 1)
  assert.equal(images[0].path, "/root/work/chart.png")
  assert.equal(images[0].mimeType, "image/png")
  assert.deepEqual(events.filter((e) => e.kind === "link").map((e) => e.url), ["https://example.com/a"])
})

test("approval detection fires only on a trailing prompt", () => {
  assert.ok(detectApproval("Overwrite the file? [y/N]"))
  assert.equal(detectApproval("Overwrite? [y/N]\nyes\nmoving on\n"), null)
  assert.deepEqual(detectApproval("1) allow\n2) deny\nChoose:").options, ["allow", "deny"])
  assert.equal(detectApproval("just some output\n"), null)
})

// -- errors -----------------------------------------------------------------

test("native CuaError variants map to stable codes", () => {
  const native = (tag) => Object.assign(new Error(`${tag}: x`), { tag })
  assert.equal(toSpacesError(native("CapabilityMissing")).code, "capability_missing")
  assert.equal(toSpacesError(native("TeleportRefused")).code, "teleport_refused")
  assert.equal(toSpacesError(native("Unauthenticated")).code, "auth")
  assert.equal(toSpacesError(native("NotFound")).code, "not_found")
  assert.equal(toSpacesError(native("AmbiguousSandbox")).code, "ambiguous_sandbox")
  assert.equal(toSpacesError(new Error("plain")).code, "protocol")
  const e = new SpacesError("usage", "u")
  assert.equal(toSpacesError(e), e)
  assert.equal(toSpacesError(native("NotFound")).cause.tag, "NotFound")
})

// -- Thread over a scripted Space ---------------------------------------------

/** A SpaceLike whose agent run follows a script of statuses. */
function scriptedSpace(statuses, tails = []) {
  const calls = []
  let i = 0
  const at = () => Math.min(i, statuses.length - 1)
  return {
    calls,
    id: () => "space://direct/scripted:1",
    async agentStart(agent, prompt, show) {
      calls.push(["start", agent, prompt, show])
      return { runId: "run-1", agent, space: "space://direct/scripted:1", processTag: "t", notes: [], json: "{}" }
    },
    async agentStatus(runId, tail) {
      calls.push(["status", runId, tail])
      const status = statuses[at()]
      const out = tails[at()] ?? ""
      i++
      return { runId, agent: "claude-code", status, reason: `because ${status}`, exitCode: undefined, acceptsMessage: ["idle", "crashed", "failed"].includes(status), outputTail: out, json: "{}" }
    },
    async agentMessage(runId, text, force) {
      calls.push(["message", runId, text, force])
      const status = statuses[at()]
      const ok = status === "idle" || force
      return { runId, ok, reason: ok ? "delivered" : `the run is ${status}`, json: "{}" }
    },
    async agentStop(runId) {
      calls.push(["stop", runId])
      return { runId, ok: true, reason: "process gone", json: "{}" }
    },
  }
}

function scriptedSpaces(space) {
  const deleted = []
  return {
    deleted,
    async space(id) {
      assert.equal(id, space.id())
      return space
    },
    async create(options) {
      return { space: { id: space.id() }, pendingId: undefined, reused: false, options }
    },
    async delete_(id) {
      deleted.push(id)
      return `Deleted ${id}`
    },
  }
}

test("startThread refuses bad input before any call", async () => {
  const space = scriptedSpace(["running"])
  const spaces = scriptedSpaces(space)
  await assert.rejects(startThread(spaces, { agent: "nope", prompt: "p", placement: { type: "dedicated" } }), (e) =>
    isSpacesError(e, "usage"),
  )
  await assert.rejects(startThread(spaces, { agent: "claude-code", prompt: "", placement: { type: "dedicated" } }), (e) =>
    isSpacesError(e, "usage"),
  )
  assert.deepEqual(space.calls, [])
})

test("a thread's events stop when the run settles, with a state event per change", async () => {
  const space = scriptedSpace(["running", "running", "idle"], ["working\n", "working\nwrote /root/out/a.md\n", "working\nwrote /root/out/a.md\ndone\n"])
  const spaces = scriptedSpaces(space)
  const thread = await startThread(spaces, {
    agent: "claude-code",
    prompt: "go",
    placement: { type: "shared", space: space.id(), acknowledgeNoIsolation: true },
  })
  assert.equal(thread.isolation, "none")
  assert.equal(thread.runId, "run-1")
  const events = []
  for await (const e of thread.events({ intervalMs: 1, maxPolls: 50 })) events.push(e)
  const states = events.filter((e) => e.kind === "state").map((e) => e.state)
  assert.deepEqual(states, ["running", "idle"])
  assert.ok(events.some((e) => e.kind === "file" && e.path === "/root/out/a.md"))
  assert.deepEqual(events.filter((e) => e.kind === "text").map((e) => e.text), ["working", "wrote /root/out/a.md", "done"])
})

test("events() is bounded even when the run never settles", async () => {
  const space = scriptedSpace(["running"])
  const thread = new Thread({ runId: "run-1", agent: "claude-code", space, spaces: scriptedSpaces(space), isolation: "space", deleteOnClose: false })
  let n = 0
  for await (const _ of thread.events({ intervalMs: 1, maxPolls: 5 })) n++
  assert.equal(space.calls.filter((c) => c[0] === "status").length, 5)
  assert.equal(n, 1, "one state event for an unchanging status")
})

test("send is refused while a turn runs and delivered once idle; stop reports what it witnessed", async () => {
  const space = scriptedSpace(["running", "idle"])
  const thread = new Thread({ runId: "run-1", agent: "claude-code", space, spaces: scriptedSpaces(space), isolation: "space", deleteOnClose: false })
  const refused = await thread.send("more")
  assert.equal(refused.state, "refused")
  assert.match(refused.reason, /running/)
  await thread.status() // advances the script to idle
  const delivered = await thread.send("more")
  assert.equal(delivered.state, "delivered")
  assert.deepEqual(await thread.stop(), { stopped: true, reason: "process gone" })
})

test("send follows the server's published acceptsMessage, not the status word", async () => {
  // A crashed run accepts (the server restarts it); an idle row the server
  // marks not accepting is refused without a message being sent.
  const crashed = scriptedSpace(["crashed"])
  const t1 = new Thread({ runId: "run-1", agent: "claude-code", space: crashed, spaces: scriptedSpaces(crashed), isolation: "space", deleteOnClose: false })
  assert.equal((await t1.status()).acceptsMessage, true)
  crashed.agentMessage = async (runId) => ({ runId, ok: true, reason: "restarted", json: "{}" })
  assert.equal((await t1.send("again")).state, "delivered")
  const space = scriptedSpace(["idle"])
  space.agentStatus = async (runId) => ({ runId, agent: "claude-code", status: "idle", reason: "held", acceptsMessage: false, json: "{}" })
  space.agentMessage = async () => assert.fail("no message when the server does not accept one")
  const t2 = new Thread({ runId: "run-1", agent: "claude-code", space, spaces: scriptedSpaces(space), isolation: "space", deleteOnClose: false })
  const refused = await t2.send("more")
  assert.deepEqual([refused.state, refused.reason], ["refused", "idle: held"])
})

test("closing a dedicated thread deletes the Space it created", async () => {
  const space = scriptedSpace(["running"])
  const spaces = scriptedSpaces(space)
  const thread = await startThread(spaces, { agent: "openai-codex", prompt: "go", placement: { type: "dedicated" } })
  assert.equal(thread.isolation, "space")
  await thread.close()
  assert.deepEqual(spaces.deleted, [space.id()])
})

// -- MCP session and transports --------------------------------------------

/** A Transport that answers from a function and records what it saw. */
function fakeTransport(answer) {
  const sent = []
  return {
    sent,
    kind: "fake",
    async send(message) {
      sent.push(message)
      if (!("id" in message)) return null
      return { jsonrpc: "2.0", id: message.id, ...answer(message) }
    },
    async close() {},
  }
}

test("McpSession handshakes once and turns isError into a ToolError with its kind", async () => {
  const t = fakeTransport((m) => {
    if (m.method === "initialize") return { result: { serverInfo: { name: "cua-spaces", version: "0" } } }
    if (m.params?.name === "stream_endpoint")
      return {
        result: {
          content: [{ type: "text", text: "error: no desktop_stream" }],
          isError: true,
          structuredContent: { error: { kind: "capability_missing", message: "no desktop_stream" } },
        },
      }
    return { result: { content: [{ type: "text", text: '[{"id":"space://direct/a:1"}]' }], isError: false } }
  })
  const s = new McpSession(t)
  assert.deepEqual(await s.callJson("list_spaces"), [{ id: "space://direct/a:1" }])
  await assert.rejects(s.call("stream_endpoint", { space: "x" }), (e) => e instanceof ToolError && e.kind === "capability_missing")
  assert.equal(t.sent.filter((m) => m.method === "initialize").length, 1)
})

test("McpSession re-initializes once when the server says the session expired", async () => {
  let expired = true
  const sent = []
  const t = {
    kind: "fake",
    async send(message) {
      sent.push(message.method)
      if (!("id" in message)) return null
      if (message.method === "tools/list" && expired) {
        expired = false
        throw new TransportError("expired", 404)
      }
      return { jsonrpc: "2.0", id: message.id, result: { tools: [{ name: "list_spaces" }] } }
    },
    async close() {},
  }
  const s = new McpSession(t)
  assert.deepEqual((await s.listTools()).map((x) => x.name), ["list_spaces"])
  assert.deepEqual(sent, ["initialize", "notifications/initialized", "tools/list", "initialize", "notifications/initialized", "tools/list"])
})

test("TauriTransport carries messages through invoke and maps a notification to null", async () => {
  const calls = []
  const invoke = async (command, args) => {
    calls.push([command, args])
    if (command !== "spaces_mcp_request") return undefined
    return "id" in args.message ? { jsonrpc: "2.0", id: args.message.id, result: {} } : null
  }
  const t = new TauriTransport({ invoke })
  assert.deepEqual(await t.send({ jsonrpc: "2.0", id: 7, method: "ping" }), { jsonrpc: "2.0", id: 7, result: {} })
  assert.equal(await t.send({ jsonrpc: "2.0", method: "notifications/initialized" }), null)
  await t.close()
  assert.deepEqual(calls.map((c) => c[0]), ["spaces_mcp_request", "spaces_mcp_request", "spaces_mcp_shutdown"])
})

/** A loopback stand-in for the daemon's `/mcp`. */
async function withMcpServer(body) {
  const seen = []
  const server = createServer(async (req, res) => {
    const chunks = []
    for await (const c of req) chunks.push(c)
    const text = Buffer.concat(chunks).toString("utf8")
    seen.push({ method: req.method, url: req.url, auth: req.headers.authorization, session: req.headers["mcp-session-id"], text })
    if (req.headers.authorization !== "Bearer daemon-token") {
      res.writeHead(401).end("no")
      return
    }
    if (req.method === "DELETE") {
      res.writeHead(200).end()
      return
    }
    const m = JSON.parse(text)
    if (m.method === "initialize") {
      res.writeHead(200, { "content-type": "application/json", "mcp-session-id": "sess-1" })
      res.end(JSON.stringify({ jsonrpc: "2.0", id: m.id, result: { serverInfo: { name: "cua-spaces", version: "0" } } }))
      return
    }
    if (req.headers["mcp-session-id"] !== "sess-1") {
      res.writeHead(404).end("unknown session")
      return
    }
    if (!("id" in m)) {
      res.writeHead(202).end()
      return
    }
    res.writeHead(200, { "content-type": "application/json" })
    res.end(JSON.stringify({ jsonrpc: "2.0", id: m.id, result: { content: [{ type: "text", text: "ok" }], isError: false } }))
  })
  await new Promise((r) => server.listen(0, "127.0.0.1", r))
  try {
    return await body(`http://127.0.0.1:${server.address().port}`, seen)
  } finally {
    await new Promise((r) => server.close(r))
  }
}

test("McpHttpTransport sends the bearer, keeps the session id, and ends it on close", async () => {
  await withMcpServer(async (url, seen) => {
    const t = new McpHttpTransport({ url, token: "daemon-token" })
    const s = new McpSession(t)
    assert.equal(await s.callText("list_spaces"), "ok")
    assert.equal(t.sessionId, "sess-1")
    await s.close()
    assert.deepEqual(seen.map((r) => r.method), ["POST", "POST", "POST", "DELETE"])
    assert.ok(seen.every((r) => r.url === "/mcp" && r.auth === "Bearer daemon-token"))
    assert.deepEqual(seen.slice(1).map((r) => r.session), ["sess-1", "sess-1", "sess-1"])
  })
})

test("McpHttpTransport reports a refused bearer as a 401 TransportError", async () => {
  await withMcpServer(async (url) => {
    const s = new McpSession(new McpHttpTransport({ url, token: "wrong" }))
    await assert.rejects(s.listTools(), (e) => e instanceof TransportError && e.code === 401)
  })
})

// -- host presentation ------------------------------------------------------

async function withControlServer(handler, body) {
  const requests = []
  const server = createServer(async (req, res) => {
    const chunks = []
    for await (const chunk of req) chunks.push(chunk)
    requests.push({ path: req.url, auth: req.headers.authorization, body: Buffer.concat(chunks).toString("utf8") })
    handler(req, res)
  })
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve))
  const token = "control-token"
  const dir = mkdtempSync(join(tmpdir(), "cua-control-"))
  const controlFile = join(dir, "spaces-control.json")
  writeFileSync(controlFile, JSON.stringify({ port: server.address().port, token }))
  try {
    return await body({ controlFile, requests, token })
  } finally {
    await new Promise((resolve) => server.close(resolve))
  }
}

const ok = (req, res) => {
  res.writeHead(200, { "content-type": "application/json" })
  res.end('{"ok":true}')
}

test("no control file means the app is not running, and that is said plainly", async () => {
  const host = new HostPresentation({ controlFile: "/nonexistent/spaces-control.json" })
  assert.equal(await host.available(), false)
  await assert.rejects(
    () => host.pin("space://local/vm"),
    (error) => isSpacesError(error, "host_unavailable") && /does not appear to be running/.test(error.message),
  )
})

test("a malformed control file is a protocol error, not a silent fallback", async () => {
  const path = join(mkdtempSync(join(tmpdir(), "cua-control-")), "spaces-control.json")
  writeFileSync(path, '{"port":"not a number"}')
  await assert.rejects(() => readControlEndpoint(path), (e) => isSpacesError(e, "protocol"))
  writeFileSync(path, "not json at all")
  await assert.rejects(() => readControlEndpoint(path), (e) => isSpacesError(e, "protocol"))
})

test("pin sends the bearer the app wrote and the space id", async () => {
  await withControlServer(ok, async ({ controlFile, requests, token }) => {
    const host = new HostPresentation({ controlFile })
    assert.equal(await host.available(), true)
    await host.pin("space://local/cua-space-abc")
    await host.pin("local:legacy")
    assert.equal(requests.length, 2)
    assert.equal(requests[0].path, "/pip/pin")
    assert.equal(requests[0].auth, `Bearer ${token}`)
    assert.deepEqual(JSON.parse(requests[0].body), { space_id: "space://local/cua-space-abc" })
  })
})

test("a non-local id is refused here rather than sent for the app to reject with 400", async () => {
  await withControlServer(ok, async ({ controlFile, requests }) => {
    const host = new HostPresentation({ controlFile })
    for (const call of [
      () => host.pin("space://fleet/ns/claim"),
      () => host.openViewer("fleet:ns:claim"),
      () => host.streamWindow({ spaceId: "space://direct/h:1", windowId: "w", rcdpToken: "t" }),
    ]) {
      await assert.rejects(call, (e) => isSpacesError(e, "usage") && /local:<vm>/.test(e.message))
    }
    assert.equal(requests.length, 0, "nothing should have been sent")
  })
})

test("streamWindow requires a freshly-read token", async () => {
  await withControlServer(ok, async ({ controlFile }) => {
    const host = new HostPresentation({ controlFile })
    await assert.rejects(
      () => host.streamWindow({ spaceId: "local:vm", windowId: "w", rcdpToken: "" }),
      (e) => isSpacesError(e, "usage") && /rotates on every rcdpd restart/.test(e.message),
    )
  })
})

test("streamWindow passes the replica flag through", async () => {
  await withControlServer(ok, async ({ controlFile, requests }) => {
    await new HostPresentation({ controlFile }).streamWindow({
      spaceId: "local:vm",
      windowId: "w1",
      rcdpToken: "tok",
      appName: "Google Chrome",
      replica: true,
    })
    assert.deepEqual(JSON.parse(requests[0].body), {
      space_id: "local:vm",
      window_id: "w1",
      rcdp_token: "tok",
      app_name: "Google Chrome",
      title: "",
      replica: true,
    })
  })
})

test("an HTTP 200 without an acknowledgement is an unproven effect, not a success", async () => {
  const silent = (req, res) => {
    res.writeHead(200, { "content-type": "application/json" })
    res.end("{}")
  }
  await withControlServer(silent, async ({ controlFile }) => {
    await assert.rejects(
      () => new HostPresentation({ controlFile }).pin("local:vm"),
      (error) => isSpacesError(error, "protocol") && /unproven/.test(error.message),
    )
  })
})

test("a control-server error status surfaces with its status code", async () => {
  const bad = (req, res) => {
    res.writeHead(400, { "content-type": "application/json" })
    res.end('{"error":"space_id required"}')
  }
  await withControlServer(bad, async ({ controlFile }) => {
    await assert.rejects(
      () => new HostPresentation({ controlFile }).unpin("local:vm"),
      (error) => isSpacesError(error, "http") && error.status === 400,
    )
  })
})

test("a control server that has gone away reads as host_unavailable", async () => {
  const { controlFile } = await withControlServer(ok, async (context) => context)
  await assert.rejects(
    () => new HostPresentation({ controlFile }).pin("local:vm"),
    (error) => isSpacesError(error, "host_unavailable") && /may\s+have restarted/.test(error.message),
  )
})

// -- CUA_HOME ------------------------------------------------------------------

test("the control file lives under CUA_HOME, like the Rust core", async () => {
  assert.equal(cuaHome("/home/u", { CUA_HOME: "/tmp/cua-home" }), "/tmp/cua-home")
  assert.equal(cuaHome("/home/u/", {}), "/home/u/.cua")
  assert.equal(cuaHome("/home/u", { CUA_HOME: "" }), "/home/u/.cua")
  // readControlEndpoint without an explicit file reads $CUA_HOME, never ~/.cua.
  const home = mkdtempSync(join(tmpdir(), "cua-home-"))
  writeFileSync(join(home, "spaces-control.json"), JSON.stringify({ port: 4321, token: "t" }))
  const saved = process.env.CUA_HOME
  process.env.CUA_HOME = home
  try {
    const ep = await readControlEndpoint()
    assert.equal(ep.port, 4321)
  } finally {
    if (saved === undefined) delete process.env.CUA_HOME
    else process.env.CUA_HOME = saved
  }
})
