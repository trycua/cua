// direct-connect, daemon-agnostic and daemon-vs-embedded (TypeScript).
// See ../python/test_{direct_connect,daemon_agnostic,daemon_vs_embedded}.py
// for the scenario definitions; the assertions here are the same.
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { randomBytes } from "node:crypto"
import { existsSync } from "node:fs"
import { after, before } from "node:test"

import * as e from "./lib.mjs"

const { cua } = e
let fx
before(async () => {
  if (!e.laneEnabled("hermetic")) fx = await e.startFixtures()
})
after(async () => fx?.stop())

const fakeFleet = () =>
  cua.embedded({
    stateDir: e.tmpState(),
    fleetFromEnv: false,
    fleet: cua.FleetSettings.create({ baseUrl: fx.fleet_base_url, token: fx.fleet_token }),
  })

// ------------------------------------------------------------ direct-connect

async function direct(c, url, token, mock) {
  const sbx = c.sandboxes()
  const sb = await sbx.connectUrl(url, token, e.name("direct"))
  try {
    assert.equal(sb.location(), "direct")
    const env = await e.waitEnv(sb, 90)
    const summary = await e.envSmoke(env, { desktop: true, mock })
    const bad = await sbx.connectUrl(url, "wrong-token", e.name("direct-bad"))
    await assert.rejects(bad.spacesd(5000), (err) => e.isErr(err, "Unauthenticated"))
    await bad.delete_()
    return summary
  } finally {
    await sb.delete_()
  }
}

e.e2eTest("direct-connect", "hermetic", "MockServer by URL + token", async () => {
  await direct(e.embeddedLocal(e.tmpState()), fx.env_url, fx.env_token, true)
})

e.e2eTest("direct-connect", "container", "spacesd in docker by URL + token", async () => {
  await e.withDriverContainer("direct", async (d) => {
    const s = await direct(e.embeddedLocal(e.tmpState()), d.url, d.token, false)
    assert.equal(s.os_family, "linux")
    assert.ok(s.screen[0] >= 640)
  })
}, { timeout: 300_000 })

// ------------------------------------------------------------ daemon-agnostic

async function plainLocal(c, image, port, banner, what, memoryMb) {
  const sbx = c.sandboxes()
  const nm = e.name(what)
  let sb
  try {
    sb = await sbx.create(cua.SandboxCreateOptions.create({
      on: "local", image, name: nm, cpus: 1, memoryMb: BigInt(memoryMb),
      ports: [port], services: new Map([["plain", port]]),
      waitFor: [cua.ReadinessProbe.create({ port })], readyTimeoutMs: 300_000,
    }))
    assert.equal((await sb.refresh()).status, cua.SandboxStatus.Running)
    assert.ok((await sbx.list("local")).some((s) => s.name === nm))
    const fwd = await sb.forward(port)
    try {
      await e.bannerVia(fwd.localAddr(), banner)
    } finally {
      await fwd.close()
    }
    await sb.waitReady([cua.ReadinessProbe.create({ port })], 30_000)
    await assert.rejects(sb.waitReady([cua.ReadinessProbe.create({ port: 3999, httpPath: "/" })], 3000))
    await assert.rejects(sb.spacesd(3000), (err) => e.isErr(err, "SpacesdNotAvailable"))
  } finally {
    await sb?.delete_()
  }
  assert.ok(!(await sbx.list("local")).some((s) => s.name === nm))
}

e.e2eTest("daemon-agnostic", "hermetic", "fake Fleet pool without env + direct URL without a driver", async () => {
  const c = fakeFleet()
  const fleet = c.fleet()
  const pool = e.name("agnostic")
  await e.applyPool(fleet, { name: pool, image: "img:plain", services: { server: 8000 } })
  let sb
  try {
    sb = await c.sandboxes().create(cua.SandboxCreateOptions.create({ on: "cloud", pool, name: `${pool}-c` }))
    assert.equal((await sb.service("server").request("GET", "/status", undefined, 5000)).status, 200)
    await assert.rejects(sb.spacesd(2000), (err) => e.isErr(err, "SpacesdNotAvailable"))
  } finally {
    await sb?.delete_()
    await fleet.deletePool(pool)
  }
  const d = await c.sandboxes().connectUrl(fx.fleet_base_url, "t", e.name("nodriver"))
  await assert.rejects(d.spacesd(2000), (err) => e.isErr(err, "SpacesdNotAvailable"))
  await d.delete_()
})

e.e2eTest("daemon-agnostic", "container", "plain ubuntu-server (sshd only)", async () => {
  e.requireImage(e.plainImage("ubuntu-server"))
  await plainLocal(e.embeddedLocal(e.tmpState()), `container:${e.plainImage("ubuntu-server")}`, 22, "SSH-2.0", "plain-ssh", 512)
})

e.e2eTest("daemon-agnostic", "container", "plain ubuntu-xfce-vnc (Xvnc only)", async () => {
  e.requireImage(e.plainImage("ubuntu-xfce-vnc"))
  await plainLocal(e.embeddedLocal(e.tmpState()), `container:${e.plainImage("ubuntu-xfce-vnc")}`, 5901, "RFB 003", "plain-vnc", 1024)
})

e.e2eTest("daemon-agnostic", "qemu", "plain ubuntu-server disk under QEMU", async () => {
  const disk = e.diskPath("ubuntu-server")
  if (!existsSync(disk)) e.skip(`missing ${disk}`)
  await plainLocal(e.embeddedLocal(e.tmpState()), `vm:${disk}`, 22, "SSH-2.0", "qemu-ssh", 1024)
}, { timeout: 900_000 })

e.e2eTest("daemon-agnostic", "fleet", "legacy computer-server image on Fleet (no spacesd)", async () => {
  const c = cua.embedded({ stateDir: e.tmpState() })
  const fleet = c.fleet()
  const pool = e.name("agnostic")
  let sb
  try {
    await e.applyPool(fleet, {
      name: pool, image: e.LEGACY_FLEET_ROOTFS, runtime: "gvisor", services: { server: 8000 },
      cpu: 1, memoryMb: 2048, ttlSecondsAfterCreated: 3600,
    })
    sb = await c.sandboxes().create(cua.SandboxCreateOptions.create({
      on: "cloud", pool, name: `${pool}-c`, readyTimeoutMs: 900_000,
    }))
    await e.poll("server /status", async () => (await sb.service("server").request("GET", "/status", undefined, 30_000)).status === 200,
      { attempts: 60, delayMs: 5000 })
    await assert.rejects(sb.spacesd(10_000), (err) => e.isErr(err, "SpacesdNotAvailable"))
  } finally {
    await sb?.delete_()
    await fleet.deletePool(pool).catch(() => {})
  }
}, { timeout: 1_500_000 })

// ------------------------------------------------------------ daemon-vs-embedded

async function script(c, target, suffix) {
  const sbx = c.sandboxes()
  const nm = e.name(`dve-${suffix}`)
  const sb = target.kind === "direct"
    ? await sbx.connectUrl(target.url, target.token, nm)
    : await sbx.create(cua.SandboxCreateOptions.create({
        on: "local", image: `container:${e.desktopImage()}`, name: nm,
        token: target.token, env: new Map([["CUA_ENV_TOKEN", target.token]]), cpus: 2, memoryMb: 2048n,
        waitFor: [cua.ReadinessProbe.create({ port: 3211 })], readyTimeoutMs: 300_000,
      }))
  try {
    const env = await e.waitEnv(sb)
    const summary = await e.envSmoke(env, { desktop: true, mock: !!target.mock })
    const info = await sbx.get(nm)
    summary.sandbox = [info.location, info.runtimeType, info.ephemeral, info.status]
    summary.listed = (await sbx.list(undefined)).some((s) => s.name === nm)
    summary.services = [...sb.services().keys()].sort()
    return summary
  } finally {
    await sb.delete_()
  }
}

async function compare(target) {
  const embedded = await script(e.embeddedLocal(e.tmpState()), target, "emb")
  await e.withDaemon(async (d) => {
    const c = d.client()
    assert.equal(c.mode(), cua.CuaMode.Daemon)
    assert.deepEqual(await script(c, target, "dmn"), embedded)

    // Two processes share one daemon-held sandbox: a child node process
    // creates it and writes a marker; this process reattaches by name.
    const nm = e.name("dve-shared")
    const marker = randomBytes(8).toString("hex")
    const peer = spawnSync(process.execPath, ["--input-type=module", "-e", `
      const cua = await import(${JSON.stringify(process.env.CUA_TS_SDK ?? `${e.CUA_ROOT}/typescript/dist/index.js`)})
      const [sock, name, kind, url, token, image, marker] = process.argv.slice(1)
      const c = cua.connect(sock)
      const sbx = c.sandboxes()
      const sb = kind === "direct" ? await sbx.connectUrl(url, token, name) : await sbx.create(cua.SandboxCreateOptions.create({
        on: "local", image: "container:" + image, name, token, env: new Map([["CUA_ENV_TOKEN", token]]),
        cpus: 2, memoryMb: 2048n, waitFor: [cua.ReadinessProbe.create({ port: 3211 })], readyTimeoutMs: 300000 }))
      let env
      for (let i = 0; i < 120 && !env; i++) { try { env = await sb.spacesd(5000) } catch { await new Promise(r => setTimeout(r, 1000)) } }
      await env.upload("/tmp/cua-e2e-shared-marker", new TextEncoder().encode(marker).buffer, undefined)
      console.log("pid", (await c.info()).daemonPid)
    `, d.socket, nm, target.kind, target.url ?? "", target.token, e.desktopImage(), marker], { encoding: "utf8", timeout: 900_000 })
    assert.equal(peer.status, 0, peer.stderr)
    const c2 = d.client()
    assert.ok(peer.stdout.includes(`pid ${(await c2.info()).daemonPid}`))
    const sbx = c2.sandboxes()
    assert.equal((await sbx.list(undefined)).filter((s) => s.name === nm).length, 1)
    const sb = await sbx.connect(nm)
    try {
      const env = await sb.spacesd(10_000)
      assert.equal(e.str(await env.download("/tmp/cua-e2e-shared-marker")), marker)
    } finally {
      await sb.delete_()
    }
  })
}

e.e2eTest("daemon-vs-embedded", "hermetic", "MockServer: identical results + shared sandbox", async () => {
  await compare({ kind: "direct", url: fx.env_url, token: fx.env_token, mock: true })
})

e.e2eTest("daemon-vs-embedded", "container", "local desktop container: identical results + shared sandbox", async () => {
  e.requireImage(e.desktopImage())
  await compare({ kind: "local", token: randomBytes(16).toString("hex") })
}, { timeout: 1_200_000 })
