// The guide scenarios in TypeScript: your-first-cloud-fleet, create-pool
// (Node), expire-pools-and-claims, run-omarchy, connect-with-viewer and
// local-container. Same definitions as ../python/test_{fleet_guides,desktop}.py.
import assert from "node:assert/strict"
import { randomBytes } from "node:crypto"
import { after, before } from "node:test"

import * as e from "./lib.mjs"

const { cua } = e
const FAKE_IMAGE = "registry.test/cua-e2e:fake"
let fx
before(async () => {
  if (!e.laneEnabled("hermetic")) fx = await e.startFixtures()
})
after(async () => fx?.stop())

const fakeFleet = () =>
  cua.embedded({ stateDir: e.tmpState(), fleetFromEnv: false,
    fleet: cua.FleetSettings.create({ baseUrl: fx.fleet_base_url, token: fx.fleet_token }) })
const liveFleet = () => cua.embedded({ stateDir: e.tmpState() })

// A Fleet 404 (and a read's 403 in a deleted namespace) is NotFound; a
// delete in a namespace that is already gone still answers 403.
const is404 = (err) => e.isErr(err, "NotFound") || (e.isErr(err, "Fleet") && /403/.test(String(err.message)))
const cleanup = (fleet, pool) => fleet.deletePool(pool).catch((err) => { if (!is404(err)) throw err })
// Fleet deletes asynchronously (finalizers): poll, bounded.
async function assertGone(fleet, pool) {
  await e.poll(`pool ${pool} deleted`, async () => {
    try {
      await fleet.getPool(pool)
      return false
    } catch (err) {
      if (is404(err)) return true
      throw err
    }
  }, { attempts: 90, delayMs: 2000, retry: () => false })
}
const claimGone = (fleet, pool, claim) =>
  e.poll(`claim ${claim} released`, async () => !(await fleet.listClaims(pool)).some((cl) => cl.name === claim),
    { attempts: 90, delayMs: 2000, retry: () => false })

// ------------------------------------------------------------ your-first-cloud-fleet

async function firstFleet(c, { image, runtime, desktop, services, command, token }) {
  const fleet = c.fleet()
  // Distinct per test: a just-deleted pool's namespace stays forbidden while Fleet finalizes it.
  const pool = e.name(token ? "first-env" : "first")
  let sb
  try {
    await e.applyPool(fleet, { name: pool, image, runtime, replicas: 1, cpu: 4, memoryMb: 4096, services,
      ttlSecondsAfterCreated: 7200, ...(command ? { command } : {}) })
    sb = await c.sandboxes().create(cua.SandboxCreateOptions.create({
      on: "cloud", pool, name: `${pool}-claim`, readyTimeoutMs: 1_200_000,
      ...(token ? { token } : {}) }))
    const [uname, png] = await desktop(sb)
    assert.match(uname, /Linux/)
    assert.ok(e.isPng(png))
  } finally {
    await sb?.delete_()
    await cleanup(fleet, pool)
  }
  await assertGone(fleet, pool)
}

e.e2eTest("your-first-cloud-fleet", "hermetic", "pool -> claim -> uname -> screenshot -> delete (fake Fleet)", async () => {
  const c = fakeFleet()
  await firstFleet(c, { image: FAKE_IMAGE, runtime: "kubevirt", services: { server: 8000 }, desktop: async (sb) => {
    assert.equal((await sb.service("server").request("GET", "/status", undefined, 5000)).status, 200)
    const d = await c.sandboxes().connectUrl(fx.env_url, fx.env_token, e.name("first-env"))
    try {
      const env = await d.spacesd(5000)
      const out = await env.run(e.cmd("echo", ["Linux", "mock"]))
      return [e.str(out.stdout), (await env.screenshot(undefined)).image]
    } finally {
      await d.delete_()
    }
  } })
})

e.e2eTest("your-first-cloud-fleet", "fleet", "the tutorial's KubeVirt pool (legacy /cmd)", async () => {
  await firstFleet(liveFleet(), { image: e.LEGACY_FLEET_IMAGE, runtime: "kubevirt", services: { server: 8000 },
    desktop: async (sb) => {
      await e.poll("computer-server", async () => (await sb.service("server").request("GET", "/status", undefined, 30_000)).status === 200,
        { attempts: 90, delayMs: 5000 })
      const out = await e.legacyCmd(sb, "server", "run_command", { command: "uname -a" })
      const shot = await e.legacyCmd(sb, "server", "screenshot")
      return [out.stdout ?? "", Buffer.from(shot.image_data ?? shot.result?.image_data, "base64")]
    } })
}, { timeout: 1_800_000 })

e.e2eTest("your-first-cloud-fleet", "fleet-env", "spacesd image on Fleet (gVisor)", async () => {
  const token = randomBytes(16).toString("hex")
  await firstFleet(liveFleet(), { image: process.env.CUA_E2E_FLEET_ENV_IMAGE, runtime: "gvisor", services: { env: 3211 },
    command: e.envTokenCommand(token), token,
    desktop: async (sb) => {
      const env = await e.waitEnv(sb, 180)
      return [e.str((await env.sh("uname -a", undefined)).stdout), (await env.screenshot(undefined)).image]
    } })
}, { timeout: 1_800_000 })

e.e2eTest("your-first-cloud-fleet", "fleet-env", "spacesd image on Fleet (KubeVirt)", async () => {
  e.skip(e.KUBEVIRT_ENV_SKIP)
})

// ------------------------------------------------------------ create-pool (Node)

async function createPool(c, { image, live }) {
  const fleet = c.fleet()
  const pool = e.name("pool")
  const claim = `${pool}-claim`
  const spec = { name: pool, image, runtime: "gvisor", replicas: 1, cpu: 1, memoryMb: 2048, services: { server: 8000 },
    ttlSecondsAfterCreated: 7200 }
  try {
    assert.equal((await e.applyPool(fleet, spec)).replicas, 1)
    assert.equal((await e.applyPool(fleet, spec)).name, pool)
    if (live) assert.ok(((await fleet.waitPoolReady(pool, 900_000)).readyReplicas ?? 0) >= 1)
    const sbx = c.sandboxes()
    const opts = cua.SandboxCreateOptions.create({ on: "cloud", pool, name: claim, readyTimeoutMs: 900_000 })
    const sb = await sbx.create(opts)
    const sb2 = await sbx.create(opts)
    assert.equal((await fleet.listClaims(pool)).filter((cl) => cl.name === claim).length, 1)
    assert.equal(sb2.name(), sb.name())
    await e.poll("server", async () => (await sb2.service("server").request("GET", "/status", undefined, 30_000)).status === 200,
      { attempts: live ? 60 : 3, delayMs: live ? 5000 : 100 })
    assert.equal((await fleet.setPoolReplicas(pool, 2)).replicas, 2)
    await sb.delete_()
    await claimGone(fleet, pool, claim)
    assert.equal((await fleet.getPool(pool)).name, pool)
  } finally {
    await cleanup(fleet, pool)
  }
}

async function ephemeral(c, image) {
  const fleet = c.fleet()
  const sb = await c.sandboxes().create(cua.SandboxCreateOptions.create({
    on: "cloud", image, runtime: "gvisor", services: new Map([["server", 8000]]),
    cpus: 1, memoryMb: 2048n, fleetTtlSeconds: 3600, readyTimeoutMs: 900_000 }))
  const pool = [...sb.info().endpoints.values()].map((u) => /\/api\/svc\/([^/]+)\//.exec(u)?.[1]).find(Boolean)
  try {
    try {
      assert.ok(sb.isEphemeral())
      // Managed pools are cua-auto-<tenant/spec hash> and outlive the sandbox.
      assert.match(pool, /^cua-auto-/)
      assert.equal((await fleet.getPool(pool)).name, pool)
    } finally {
      await sb.delete_()
    }
    await e.poll(`claims of ${pool} released`, async () => (await fleet.listClaims(pool)).length === 0,
      { attempts: 90, delayMs: 2000 })
    assert.equal((await fleet.getPool(pool)).name, pool, "the managed pool stays for reuse")
  } finally {
    // Scoped GC through the SDK: only this pool, only once it has no claims.
    const report = await fleet.pools().gcPools([pool], 0)
    assert.deepEqual(report.errors, [])
  }
  await assertGone(fleet, pool)
}

e.e2eTest("create-pool", "hermetic", "warm pool, named claim reattach, scaling, ephemeral cleanup (fake Fleet)", async () => {
  const c = fakeFleet()
  await createPool(c, { image: FAKE_IMAGE, live: false })
  await ephemeral(c, FAKE_IMAGE)
})

e.e2eTest("create-pool", "fleet", "warm pool, named claim reattach, scaling", async () => {
  await createPool(liveFleet(), { image: e.LEGACY_FLEET_ROOTFS, live: true })
}, { timeout: 1_800_000 })

e.e2eTest("create-pool", "fleet", "ephemeral sandbox on a managed cua-auto-* pool; the test GCs the pool", async () => {
  await ephemeral(liveFleet(), e.LEGACY_FLEET_ROOTFS)
}, { timeout: 1_800_000 })

// ------------------------------------------------------------ expire-pools-and-claims

function hasTtl(json, seconds) {
  let found = false
  JSON.parse(json, (k, v) => {
    if ((k === "ttlSecondsAfterCreated" || k === "ttl_seconds_after_created") && v === seconds) found = true
    return v
  })
  return found
}

async function expire(c, { image, live }) {
  const fleet = c.fleet()
  const pool = e.name("ttl")
  const spec = { name: pool, image, runtime: "gvisor", replicas: live ? 0 : 1, cpu: 1, memoryMb: 1024,
    services: { server: 8000 }, ttlSecondsAfterCreated: 86400 }
  try {
    await e.applyPool(fleet, spec)
    const got = await fleet.getPool(pool)
    assert.ok(hasTtl(got.json, 86400), got.json.slice(0, 400))
    const claim = await fleet.claim(pool, `${pool}-claim`, 3600)
    const body = (await fleet.listClaims(pool)).find((cl) => cl.name === claim.name).json
    assert.ok(hasTtl(body, 3600) || body.includes("shutdownTime"), body.slice(0, 400))
    await fleet.release(pool, claim.name)
  } finally {
    await cleanup(fleet, pool)
  }
}

e.e2eTest("expire-pools-and-claims", "hermetic", "pool + claim TTLs reach Fleet (fake)", async () => {
  await expire(fakeFleet(), { image: FAKE_IMAGE, live: false })
})

e.e2eTest("expire-pools-and-claims", "fleet", "pool + claim TTLs reach Fleet", async () => {
  await expire(liveFleet(), { image: e.LEGACY_FLEET_ROOTFS, live: true })
}, { timeout: 900_000 })

// ------------------------------------------------------------ desktop: local-container, run-omarchy, viewer

async function withLocalDesktop(fn) {
  e.requireImage(e.desktopImage())
  const c = e.embeddedLocal(e.tmpState())
  const token = randomBytes(16).toString("hex")
  const sb = await c.sandboxes().create(cua.SandboxCreateOptions.create({
    on: "local", image: `container:${e.desktopImage()}`, name: e.name("desktop"), token,
    env: new Map([["CUA_ENV_TOKEN", token]]), cpus: 2, memoryMb: 2048n,
    services: new Map([["env", 3211]]),
    waitFor: [cua.ReadinessProbe.create({ port: 3211, httpPath: "/viewer/" })], readyTimeoutMs: 300_000 }))
  try {
    return await fn({ c, sb, token })
  } finally {
    await sb.delete_()
  }
}

async function omarchy({ sb, token }) {
  const env = await e.waitEnv(sb)
  await e.desktopChecks(env)
  const fwd = await sb.forward(3211)
  try {
    const init = await e.mcpInitialize(fwd.localAddr(), token)
    assert.ok(init.serverInfo.name)
  } finally {
    await fwd.close()
  }
}

async function viewer({ sb }) {
  const r = await e.poll("viewer /viewer/", async () => {
    const x = await sb.service("env").request("GET", "/viewer/", undefined, 30_000)
    return x.status === 200 ? x : undefined
  }, { attempts: 30, delayMs: 2000 })
  assert.match(e.str(r.body), /viewer\.js/)
  const link = await sb.viewerUrl(cua.ViewerOptions.create({ ttlSeconds: 600, viewOnly: true }))
  assert.ok(link.url.includes("/viewer/#ticket="), link.url)
  assert.ok(link.expiresAtUnix > 0n)
  const fwd = await sb.forward(3211)
  try {
    const page = await fetch(`http://${fwd.localAddr()}/viewer/`)
    assert.equal(page.status, 200)
    assert.match(await page.text(), /viewer\.js/)
  } finally {
    await fwd.close()
  }
}

e.e2eTest("local-container", "container", "desktop image as a local container sandbox + suspend/resume", async () => {
  await withLocalDesktop(async ({ sb }) => {
    // gVisor when Docker has it (the CI runner installs it), else runc.
    assert.equal(sb.runtimeType(), e.hasRunsc() ? "gvisor" : "container")
    await e.envSmoke(await e.waitEnv(sb), { desktop: true })
    await sb.suspend()
    await sb.resume()
    const env = await e.waitEnv(sb)
    assert.equal(e.str((await env.run(e.cmd("echo", ["back"]))).stdout), "back\n")
    if (e.hasRunsc()) assert.equal(e.docker(["inspect", "--format", "{{.HostConfig.Runtime}}", sb.name()]).stdout.trim(), "runsc")
  })
}, { timeout: 600_000 })

e.e2eTest("run-omarchy", "container", "dimensions, clipboard, click + keys in the grid fixture, /mcp initialize", async () => {
  await withLocalDesktop(omarchy)
}, { timeout: 600_000 })

e.e2eTest("connect-with-viewer", "container", "env service /viewer/ page + forward + viewerUrl ticket link", async () => {
  await withLocalDesktop(viewer)
}, { timeout: 600_000 })

e.e2eTest("run-omarchy", "fleet-env", "desktop checks on a Fleet gVisor pool of the spacesd image", async () => {
  const c = liveFleet()
  const fleet = c.fleet()
  const pool = e.name("omarchy")
  const token = randomBytes(16).toString("hex")
  let sb
  try {
    await e.applyPool(fleet, { name: pool, image: process.env.CUA_E2E_FLEET_ENV_IMAGE, runtime: "gvisor", cpu: 2,
      memoryMb: 4096, services: { env: 3211 }, command: e.envTokenCommand(token), ttlSecondsAfterCreated: 7200 })
    sb = await c.sandboxes().create(cua.SandboxCreateOptions.create({
      on: "cloud", pool, name: `${pool}-c`, token, readyTimeoutMs: 1_200_000 }))
    await e.desktopChecks(await e.waitEnv(sb))
  } finally {
    await sb?.delete_()
    await cleanup(fleet, pool)
  }
  await assertGone(fleet, pool)
}, { timeout: 1_800_000 })

// ------------------------------------------------------------ local-qemu (SDK local provider)

e.e2eTest("local-qemu", "qemu", "reference disk via vm:<disk>: probe, forward, suspend/resume", async () => {
  const { existsSync } = await import("node:fs")
  // CUA_E2E_DISK_CUA_DESKTOP_LINUX: the pre-rename name, read for one release.
  const disk = process.env.CUA_E2E_DISK_LINUX ?? process.env.CUA_E2E_DISK_CUA_DESKTOP_LINUX ??
    `${process.env.HOME}/.cache/cua-images-e2e/linux/${e.HOST_ARCH}/disk.img`
  if (!existsSync(disk)) e.skip(`missing ${disk}`)
  const c = e.embeddedLocal(e.tmpState())
  const token = randomBytes(16).toString("hex")
  const sb = await c.sandboxes().create(cua.SandboxCreateOptions.create({
    on: "local", image: `vm:${disk}`, name: e.name("vm"), token,
    env: new Map([["CUA_ENV_TOKEN", token]]), cpus: 2, memoryMb: 3072n, ports: [3211],
    waitFor: [cua.ReadinessProbe.create({ port: 3211, httpPath: "/viewer/" })],
    readyTimeoutMs: 600_000 }))
  try {
    assert.equal(sb.runtimeType(), "qemu")
    const fwd = await sb.forward(3211)
    try {
      const r = await fetch(`http://${fwd.localAddr()}/viewer/`)
      assert.equal(r.status, 200)
      assert.match(await r.text(), /viewer\.js/)
    } finally {
      await fwd.close()
    }
    await sb.suspend()
    await sb.resume()
    await sb.waitReady([cua.ReadinessProbe.create({ port: 3211, httpPath: "/viewer/" })], 120_000)
  } finally {
    await sb.delete_()
  }
}, { timeout: 1_200_000 })

// ------------------------------------------------------------ images / image-build-push-run

const imageSpec = (nm, marker) => JSON.stringify({
  apiVersion: "images.cua.ai/v1alpha1", kind: "Image", metadata: { name: nm, namespace: nm },
  spec: { recipe: { osType: "linux", distro: "ubuntu", version: "24.04", kind: "vm",
    layers: [{ type: "run", command: `mkdir -p /opt/cua-e2e && echo ${marker} > /opt/cua-e2e/marker` }],
    env: { CUA_E2E_BUILT: marker }, ports: [8080] } },
})

async function withRegistry(tag, fn) {
  const nm = e.name(`registry-${tag}`)
  e.docker(["rm", "-f", nm], { check: false })
  try {
    e.docker(["run", "-d", "--name", nm, "--memory=256m", "-p", "127.0.0.1::5000", "registry:2"])
    const host = `127.0.0.1:${e.docker(["port", nm, "5000/tcp"]).stdout.trim().split("\n")[0].split(":").pop()}`
    await e.poll("registry", async () => (await fetch(`http://${host}/v2/`)).status === 200, { attempts: 30 })
    process.env.CUA_INSECURE_REGISTRIES = host
    return await fn(host)
  } finally {
    delete process.env.CUA_INSECURE_REGISTRIES
    e.docker(["rm", "-f", nm], { check: false })
  }
}

e.e2eTest("images", "container", "SDK build_image layers apply; the pushed rootfs runs as a sandbox", async () => {
  const base = e.plainImage("ubuntu-server")
  e.requireImage(base)
  const marker = randomBytes(6).toString("hex")
  await withRegistry("sdk", async (host) => {
    const c = e.embeddedLocal(e.tmpState())
    const dest = `${host}/cua-e2e/sdk-built:${e.RUN}-ts`
    const ref = await c.local().buildImage(imageSpec(e.name("img-sdk"), marker), `container:${base}`, dest)
    assert.match(ref, /@sha256:/)
    const nm = e.name("built")
    const sb = await c.sandboxes().create(cua.SandboxCreateOptions.create({
      on: "local", image: `container:${dest}`, name: nm, cpus: 1, memoryMb: 512n, readyTimeoutMs: 300_000 }))
    try {
      const out = e.docker(["exec", nm, "sh", "-c", "cat /opt/cua-e2e/marker; . /etc/profile.d/cua-env.sh; echo $CUA_E2E_BUILT"]).stdout.split(/\s+/).filter(Boolean)
      assert.deepEqual(out, [marker, marker])
    } finally {
      await sb.delete_()
    }
  })
}, { timeout: 1_200_000 })

e.e2eTest("images", "fleet", "remote build via create_image, or a typed error", async () => {
  const c = liveFleet()
  const fleet = c.fleet()
  const pool = e.name("img")
  await e.applyPool(fleet, { name: pool, image: e.LEGACY_FLEET_ROOTFS, runtime: "gvisor", replicas: 0, ttlSecondsAfterCreated: 3600 })
  try {
    const s = JSON.parse(imageSpec(e.name("img-remote"), "remote"))
    s.metadata.namespace = pool
    try {
      await fleet.createImage(pool, JSON.stringify(s))
      await fleet.deleteImage(pool, s.metadata.name)
    } catch (err) {
      assert.ok(["Fleet", "Unsupported", "InvalidArgument", "PermissionDenied"].some((k) => e.isErr(err, k)), String(err))
      assert.ok(String(err.message).length > 0)
    }
  } finally {
    await cleanup(fleet, pool)
  }
}, { timeout: 600_000 })
