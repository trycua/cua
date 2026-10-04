// The guide scenarios in TypeScript: run-omarchy, connect-with-viewer,
// local-container, local-qemu and images. Same definitions as
// ../python/test_{desktop,local_vm,images}.py.
import assert from "node:assert/strict"
import { randomBytes } from "node:crypto"

import * as e from "./lib.mjs"

const { cua } = e

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
