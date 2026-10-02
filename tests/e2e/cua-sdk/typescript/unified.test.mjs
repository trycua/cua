// unified-sandbox-api (TypeScript). See ../python/test_unified_api.py for
// the scenario; the assertions here are the same.
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { join } from "node:path"
import { after, before } from "node:test"

import * as e from "./lib.mjs"

const { cua } = e
const SERVER = readFileSync(join(e.SUITE, "fixtures", "mcp_probe_server.py"), "utf8")
const ACCEPT = "application/json, text/event-stream"

let fx
before(async () => {
  if (!e.laneEnabled("hermetic")) fx = await e.startFixtures()
})
after(async () => fx?.stop())

const h = (name, value) => cua.HttpHeader.create({ name, value })
const json = (o) => e.u8(JSON.stringify(o)).buffer
const rpc = (method, id, params) => json({ jsonrpc: "2.0", id, method, ...(params ? { params } : {}) })
const join_ = (url, path) => {
  const u = new URL(url)
  u.pathname = u.pathname.replace(/\/$/, "") + path
  return u.toString()
}

/** A plain HTTP client with no SDK credentials. */
async function plainChecks(url) {
  const health = await fetch(join_(url, "/health"))
  assert.equal(health.status, 200, await health.text())
  const init = await fetch(join_(url, "/mcp"), {
    method: "POST",
    headers: { accept: ACCEPT, "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize" }),
  })
  assert.equal(init.status, 200, await init.text())
  assert.ok(init.headers.get("mcp-session-id"))
}

async function mcpThroughService(sb, envExpected) {
  const svc = sb.service("mcp")
  const ct = h("content-type", "application/json")
  const bare = await svc.request("POST", "/mcp", rpc("x", 0), 30_000, [ct])
  assert.equal(bare.status, 400, "the server refuses dropped headers")
  const init = await svc.request("POST", "/mcp", rpc("initialize", 1), 30_000, [ct, h("accept", ACCEPT)])
  assert.equal(init.status, 200)
  const sid = init.headers.find((x) => x.name.toLowerCase() === "mcp-session-id").value
  const heads = [ct, h("accept", ACCEPT), h("mcp-session-id", sid)]
  const call = await svc.request("POST", "/mcp", rpc("tools/call", 2, { name: "add", arguments: { a: 2, b: 3 } }), 30_000, heads)
  assert.equal(JSON.parse(e.str(call.body)).result.content[0].text, "5")
  if (envExpected) {
    const env = await svc.request("POST", "/mcp", rpc("tools/call", 3, { name: "env" }), 30_000, heads)
    assert.equal(JSON.parse(e.str(env.body)).result.content[0].text, "hello")
  }
}

async function urlsAndForward(sb, port) {
  await plainChecks(await sb.service("mcp").url())
  const pub = await sb.publicUrl("mcp", 600, "e2e")
  assert.ok(pub.expiresAtUnix > 0n)
  await plainChecks(pub.url)
  const fwd = await sb.forward(port)
  try {
    assert.match(fwd.url(), /^http:\/\/127\.0\.0\.1:/)
    await plainChecks(fwd.url())
  } finally {
    await fwd.close()
  }
  await sb.revokePublicUrl(pub.id)
}

e.e2eTest("unified-sandbox-api", "hermetic", "fake Fleet command/services/URLs + daemon public URL", async () => {
  const c = cua.embedded({
    stateDir: e.tmpState(),
    fleetFromEnv: false,
    fleet: cua.FleetSettings.create({ baseUrl: fx.fleet_base_url, token: fx.fleet_token }),
  })
  const sb = await c.sandboxes().create(cua.SandboxCreateOptions.create({
    on: "cloud",
    image: "registry.example/mcp:docker-e2e",
    command: ["python", "/srv.py"],
    services: new Map([["mcp", 8765]]),
    runtime: "gvisor",
    cloud: cua.CloudOptions.create({ maxPoolSize: 2 }),
    readyTimeoutMs: 120_000,
  }))
  const pool = sb.info().providerDetails.get("pool") ?? ""
  try {
    const info = sb.info()
    assert.equal(info.location, "cloud")
    assert.equal(info.phase, cua.SandboxPhase.Ready)
    assert.ok(info.id && pool.startsWith("cua-auto-"), pool)
    assert.ok((await sb.service("mcp").url()).startsWith("https://signed.fleet.test/"))
    const p = await sb.publicUrl("mcp", 600, undefined)
    assert.ok(p.url.startsWith("https://signed.fleet.test/") && p.providerDetails.has("claim"))
    // The same from the service handle (as in Python and Rust).
    const sp = await sb.service("mcp").publicUrl(600, undefined)
    assert.ok(sp.url.startsWith("https://signed.fleet.test/") && sp.service === "mcp", sp.url)
    const r = await sb.service("mcp").request("POST", "/mcp", json({}), 30_000, [h("accept", ACCEPT), h("mcp-session-id", "s")])
    assert.equal(r.status, 200)
    const fwd = await sb.forward(8765)
    try {
      const res = await fetch(fwd.url() + "/x")
      assert.equal(res.status, 200)
      assert.match(await res.text(), /-mcp\/x/)
    } finally {
      await fwd.close()
    }
  } finally {
    await sb.delete_()
    if (pool) await c.fleet().pools().gcPools([pool], 0)
  }

  await e.withDaemon(async (d) => {
    const direct = await d.client().sandboxes().connectUrl(fx.env_url, fx.env_token, undefined)
    assert.equal(await direct.service("env").url(), fx.env_url.replace(/\/$/, ""))
    const pub = await direct.publicUrl("env", 120, undefined)
    assert.match(pub.url, /^http:\/\/127\.0\.0\.1:\d+\/s\//)
    const svcPub = await direct.service("env").publicUrl(120, "svc")
    assert.match(svcPub.url, /^http:\/\/127\.0\.0\.1:\d+\/s\//)
    await direct.revokePublicUrl(svcPub.id)
    const shared = await c.sandboxes().connectUrl(pub.url, fx.env_token, undefined)
    const out = await (await shared.spacesd(5000)).sh("echo shared", undefined)
    assert.equal(e.str(out.stdout), "shared\n")
    await direct.revokePublicUrl(pub.id)
    assert.equal((await fetch(pub.url)).status, 404)
  })
}, { timeout: 300_000 })

e.e2eTest("unified-sandbox-api", "container", "python:3.12-slim + command/services on gVisor through the daemon", async () => {
  if (e.docker(["image", "inspect", "python:3.12-slim"], { check: false }).status !== 0) {
    e.docker(["pull", "python:3.12-slim"])
  }
  await e.withDaemon(async (d) => {
    const sb = await d.client().sandboxes().create(cua.SandboxCreateOptions.create({
      on: "local",
      image: "container:python:3.12-slim",
      name: e.name("unified"),
      cpus: 1,
      memoryMb: 512n,
      command: ["python", "-c", SERVER],
      env: new Map([["GREETING", "hello"]]),
      services: new Map([["mcp", 8765]]),
      waitFor: [cua.ReadinessProbe.create({ service: "mcp", httpPath: "/health" })],
      readyTimeoutMs: 300_000,
    }))
    try {
      const info = sb.info()
      assert.equal(info.location, "local")
      assert.equal(info.phase, cua.SandboxPhase.Ready)
      assert.equal(info.services.get("mcp"), 8765)
      await mcpThroughService(sb, true)
      await urlsAndForward(sb, 8765)
    } finally {
      await sb.delete_()
    }
  })
}, { timeout: 600_000 })
