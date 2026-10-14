// Shared helpers for the cua SDK e2e suite (TypeScript / Node). Mirrors
// ../python/e2e.py: cua-e2e-<run>-* names, env-gated lanes, bounded polls,
// results appended to $CUA_E2E_RESULTS/ts.jsonl.
import { execFileSync, spawn, spawnSync } from "node:child_process"
import { randomBytes } from "node:crypto"
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs"
import { createConnection } from "node:net"
import { arch, homedir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"

export const SUITE = resolve(dirname(fileURLToPath(import.meta.url)), "..")
export const REPO = resolve(SUITE, "..", "..", "..")
export const CUA_ROOT = join(REPO, "libs", "cua")
const sdkEntry = process.env.CUA_TS_SDK ?? join(CUA_ROOT, "typescript", "dist", "index.js")
export const cua = await import(sdkEntry)

export const RUN = process.env.CUA_E2E_RUN || randomBytes(3).toString("hex")
process.env.CUA_E2E_RUN = RUN
export const HOST_ARCH = arch() === "arm64" ? "arm64" : "amd64"

export const LEGACY_FLEET_IMAGE =
  process.env.CUA_E2E_FLEET_IMAGE ??
  "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04@sha256:82702ebdd32d1f8fc05f2ea409a7c67d0ba9f8f8e4e9f1a89ce40989d5f4475d"
export const LEGACY_FLEET_ROOTFS =
  process.env.CUA_E2E_FLEET_ROOTFS ?? "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-main-809e3f81"

export const desktopImage = () =>
  process.env.CUA_E2E_DESKTOP_IMAGE ?? `cua-e2e-local/linux:docker-local-${HOST_ARCH}`
export const plainImage = (n) =>
  process.env[n === "ubuntu-server" ? "CUA_E2E_PLAIN_SERVER_IMAGE" : "CUA_E2E_PLAIN_VNC_IMAGE"] ??
  `cua-e2e-local/${n}:docker-local-${HOST_ARCH}`
export const diskPath = (image) =>
  process.env[`CUA_E2E_DISK_${image.toUpperCase().replaceAll("-", "_")}`] ??
  join(homedir(), ".cache", "cua-images-e2e", image, HOST_ARCH, "disk.img")

/** `cua-e2e-<run>-<what>-ts` (DNS label). */
export const name = (what) => `cua-e2e-${RUN}-${what}-ts`.slice(0, 63).replace(/-+$/, "")

export function binary(env, exe) {
  if (process.env[env]) return process.env[env]
  for (const profile of ["debug", "release"]) {
    const p = join(CUA_ROOT, "target", profile, exe)
    if (existsSync(p)) return p
  }
  return undefined
}

const flag = (v) => process.env[v] === "1"
const hasDocker = () => spawnSync("docker", ["version", "--format", "{{.Server.Version}}"]).status === 0

/** `undefined` when `lane` can run here, else the skip reason. */
export function laneEnabled(lane) {
  switch (lane) {
    case "hermetic":
      return binary("CUA_TEST_FIXTURES", "cua-test-fixtures") ? undefined : "cua-test-fixtures is not built"
    case "container":
      if (!flag("CUA_E2E_CONTAINER")) return "set CUA_E2E_CONTAINER=1 for the container lane"
      return hasDocker() ? undefined : "no docker engine"
    case "qemu":
      return flag("CUA_E2E_QEMU") ? undefined : "set CUA_E2E_QEMU=1 for the QEMU lane"
    case "lume":
      return flag("CUA_E2E_LUME") ? undefined : "set CUA_E2E_LUME=1 for the Lume lane"
    case "fleet":
      if (!flag("CUA_E2E_FLEET")) return "set CUA_E2E_FLEET=1 for live Fleet"
      return process.env.FLEETS_TOKEN || process.env.CUA_CLIENT_ID ? undefined : "no Fleet credentials"
    case "fleet-env":
      return (
        laneEnabled("fleet") ??
        (process.env.CUA_E2E_FLEET_ENV_IMAGE
          ? undefined
          : "CUA_E2E_FLEET_ENV_IMAGE is unset: no linux (spacesd) image in a registry Fleet can pull")
      )
    case "cua-sandbox":
      return flag("CUA_E2E_CUA_SANDBOX") ? undefined : "set CUA_E2E_CUA_SANDBOX=1"
    default:
      throw new Error(`unknown lane ${lane}`)
  }
}

// Fleet pools have no env/secret field yet: a gVisor pod of the spacesd
// image gets its token through an entrypoint override (the image reads
// /etc/cua/env-token). KubeVirt containerDisk guests have no such hook.
export const KUBEVIRT_ENV_SKIP =
  "KubeVirt spacesd lane needs a per-claim secret field (cloud PR): no way to deliver the env token to a containerDisk guest yet"
export const envTokenCommand = (token) => ["/bin/sh", "-c",
  `mkdir -p /etc/cua && printf %s ${token} >/etc/cua/env-token && exec /opt/cua/desktop/entrypoint.sh`]

export class Skip extends Error {}
export const skip = (reason) => {
  throw new Skip(reason)
}

function record(scenario, lane, title, status, secs, reason = "") {
  const out = process.env.CUA_E2E_RESULTS
  if (!out) return
  mkdirSync(out, { recursive: true })
  appendFileSync(
    join(out, "ts.jsonl"),
    JSON.stringify({ scenario, lang: "ts", lane, test: title, status, secs: Math.round(secs * 10) / 10, reason, run: RUN }) + "\n",
  )
}

/**
 * One e2e test: skipped (with the reason) when the lane is off, timed and
 * recorded either way. `opts.xfail` marks a known bug: failing is expected,
 * passing is reported as `xpass` and fails the test (strict).
 */
export function e2eTest(scenario, lane, title, fn, opts = {}) {
  const why = laneEnabled(lane)
  const timeout = opts.timeout ?? 600_000
  test(`[${scenario}/${lane}] ${title}`, { skip: why, timeout }, async (t) => {
    const t0 = performance.now()
    const secs = () => (performance.now() - t0) / 1000
    try {
      await fn(t)
    } catch (e) {
      if (e instanceof Skip) {
        record(scenario, lane, title, "skip", secs(), e.message)
        t.skip(e.message)
        return
      }
      if (opts.xfail) {
        record(scenario, lane, title, "xfail", secs(), opts.xfail)
        t.todo(`xfail: ${opts.xfail}`)
        return
      }
      record(scenario, lane, title, "fail", secs(), String(e?.message ?? e).slice(0, 400))
      throw e
    }
    if (opts.xfail) {
      record(scenario, lane, title, "xpass", secs(), opts.xfail)
      throw new Error(`xpass: ${opts.xfail} (the bug is fixed; drop the xfail)`)
    }
    record(scenario, lane, title, "pass", secs())
  })
  if (why) record(scenario, lane, title, "skip", 0, why)
}

// ---------------------------------------------------------------- misc

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

/** Awaits `fn()` until truthy (bounded). Errors not matched by `retry` propagate. */
export async function poll(what, fn, { attempts = 60, delayMs = 1000, retry = () => true } = {}) {
  let last
  for (let i = 0; i < attempts; i++) {
    try {
      last = await fn()
      if (last) return last
    } catch (e) {
      if (!retry(e)) throw e
      last = e
    }
    await sleep(delayMs)
  }
  throw new Error(`timed out waiting for ${what}; last=${last?.message ?? JSON.stringify(last)}`)
}

export const isErr = (e, kind) => cua.CuaError[kind].instanceOf(e)
const envRetry = (e) => isErr(e, "SpacesdNotAvailable") || isErr(e, "Transport") || isErr(e, "Timeout")
export const waitEnv = (sb, attempts = 120) =>
  poll("spacesd", () => sb.spacesd(5000), { attempts, delayMs: 1000, retry: envRetry })

export const u8 = (s) => new TextEncoder().encode(s)
export const str = (b) => new TextDecoder().decode(b instanceof ArrayBuffer ? new Uint8Array(b) : b)
export const bytes = (b) => (b instanceof ArrayBuffer ? new Uint8Array(b) : b)
export const isPng = (b) => {
  const a = bytes(b)
  return a[0] === 0x89 && a[1] === 0x50 && a[2] === 0x4e && a[3] === 0x47
}
export const pngSize = (b) => {
  const v = new DataView(bytes(b).buffer, bytes(b).byteOffset)
  return [v.getUint32(16), v.getUint32(20)]
}
export const cmd = (program, args = [], extra = {}) => cua.SpacesdCommand.create({ program, args, ...extra })

export function readBanner(addr, n, timeoutMs = 5000) {
  const [host, port] = [addr.slice(0, addr.lastIndexOf(":")), Number(addr.slice(addr.lastIndexOf(":") + 1))]
  return new Promise((resolveP) => {
    let buf = Buffer.alloc(0)
    const s = createConnection({ host, port })
    const done = () => {
      s.destroy()
      resolveP(buf)
    }
    s.setTimeout(timeoutMs, done)
    s.on("data", (d) => {
      buf = Buffer.concat([buf, d])
      if (buf.length >= n) done()
    })
    s.on("error", done)
    s.on("end", done)
  })
}

export async function bannerVia(addr, banner) {
  return poll(`${banner} banner`, async () => {
    const got = await readBanner(addr, banner.length)
    return got.toString("latin1").startsWith(banner) ? got : undefined
  }, { attempts: 60, delayMs: 1000 })
}

// ---------------------------------------------------------------- docker

export const docker = (args, { check = true } = {}) => {
  const r = spawnSync("docker", args, { encoding: "utf8", timeout: 120_000 })
  if (check && r.status !== 0) throw new Error(`docker ${args.join(" ")}: ${r.stderr}`)
  return r
}
export const hasRunsc = () => docker(["info", "--format", "{{json .Runtimes}}"], { check: false }).stdout.includes("runsc")
export function requireImage(ref) {
  if (docker(["image", "inspect", ref], { check: false }).status !== 0)
    skip(`image ${ref} is not present; build it with libs/images/build.sh`)
}

/** linux via plain `docker run` (the direct-connect topology). */
export async function withDriverContainer(what, fn, image = desktopImage()) {
  requireImage(image)
  const runtime = hasRunsc() ? "runsc" : "runc"
  const c = { name: name(what), token: randomBytes(16).toString("hex"), runtime, ports: {} }
  docker(["rm", "-f", c.name], { check: false })
  try {
    docker(["run", "-d", "--name", c.name, `--runtime=${runtime}`, "--memory=2g", "--memory-swap=2g",
      "--shm-size=512m", "--label", `cua-e2e-run=${RUN}`, "-e", `CUA_ENV_TOKEN=${c.token}`,
      "-p", "127.0.0.1::3211", image])
    for (const p of [3211]) {
      c.ports[p] = Number(docker(["port", c.name, `${p}/tcp`]).stdout.trim().split("\n")[0].split(":").pop())
    }
    c.url = `http://127.0.0.1:${c.ports[3211]}`
    return await fn(c)
  } finally {
    docker(["rm", "-f", c.name], { check: false })
  }
}

// ---------------------------------------------------------------- fixtures + daemon

export async function startFixtures() {
  const path = binary("CUA_TEST_FIXTURES", "cua-test-fixtures")
  const child = spawn(path, [], { stdio: ["pipe", "pipe", "inherit"] })
  const line = await new Promise((res, rej) => {
    let acc = ""
    child.stdout.on("data", (d) => {
      acc += d
      if (acc.includes("\n")) res(acc.split("\n")[0])
    })
    child.once("exit", () => rej(new Error("cua-test-fixtures exited early")))
  })
  return {
    ...JSON.parse(line),
    async stop() {
      child.stdin.end()
      await Promise.race([new Promise((r) => child.once("exit", r)), sleep(10_000).then(() => child.kill())])
    },
  }
}

export async function withDaemon(fn) {
  const cli = binary("CUA_CLI", "cua")
  if (!cli) skip("the cua CLI is not built")
  const home = mkdtempSync(`/tmp/cua-e2e-${RUN}-`)
  const sock = join(home, "cua.sock")
  const child = spawn(cli, ["daemon", "start", "--foreground", "--socket", sock, "--state-dir", join(home, "sandboxes")], {
    env: { ...process.env, CUA_HOME: home },
    stdio: ["ignore", "ignore", "inherit"],
  })
  try {
    await poll("daemon socket", async () => existsSync(sock), { attempts: 150, delayMs: 100 })
    return await fn({ socket: sock, client: () => cua.connect(sock), pid: child.pid })
  } finally {
    try {
      await cua.connect(sock).shutdownDaemon()
    } catch {}
    await Promise.race([new Promise((r) => child.once("exit", r)), sleep(15_000).then(() => child.kill())])
    rmSync(home, { recursive: true, force: true })
  }
}

// ---------------------------------------------------------------- env smoke

export async function envSmoke(env, { desktop, mock = false }) {
  const caps = await env.capabilities()
  if (!caps.version) throw new Error("no version")
  const out = await env.run(cmd("echo", ["hi"]))
  if (!out.exit.success || str(out.stdout) !== "hi\n") throw new Error(`echo: ${str(out.stdout)}`)
  const fail = await env.sh(mock ? "fail 4" : "exit 4", undefined)
  if (fail.exit.code !== 4) throw new Error(`exit code ${fail.exit.code}`)
  const p = await env.spawn(cmd("cat", [], { stdin: true }))
  await p.writeStdin(u8("xyz").buffer)
  await p.closeStdin()
  if (str((await p.wait()).stdout) !== "xyz") throw new Error("stdin roundtrip")
  const blob = new Uint8Array(1 << 20).map((_, i) => i % 251)
  const path = `/tmp/cua-e2e-${RUN}/blob-ts.bin`
  const up = await env.upload(path, blob.buffer, undefined)
  if (up.size !== BigInt(blob.length)) throw new Error("upload size")
  const down = bytes(await env.download(path))
  if (down.length !== blob.length || down.some((v, i) => v !== blob[i])) throw new Error("download mismatch")
  let missing = false
  try {
    await env.download("/definitely/not/here")
  } catch (e) {
    missing = isErr(e, "NotFound")
  }
  if (!missing) throw new Error("download of a missing file must be NotFound")
  const health = JSON.parse(await env.callJson("SystemService/Health", "{}"))
  const summary = {
    echo: str(out.stdout), exit4: fail.exit.code, stdin_roundtrip: "xyz", blob_size: Number(up.size),
    health_keys: Object.keys(health).sort(), os_family: caps.osFamily,
  }
  if (desktop) {
    await env.setClipboard("cua-e2e clipboard")
    if ((await env.getClipboard()) !== "cua-e2e clipboard") throw new Error("clipboard")
    const shot = await env.screenshot(undefined)
    if (!isPng(shot.image) || shot.width <= 0) throw new Error("screenshot")
    Object.assign(summary, { clipboard: "cua-e2e clipboard", screenshot_png: true, screen: [shot.width, shot.height] })
  }
  return summary
}

// ---------------------------------------------------------------- desktop checks

export async function shOk(env, line, timeoutMs = 60_000) {
  const out = await env.sh(line, timeoutMs)
  if (!out.exit.success) throw new Error(`${line}: ${str(out.stderr).slice(-400)}`)
  return str(out.stdout)
}

export async function fixtureLog(env, fixture) {
  try {
    return str(await env.download(`/tmp/cua-fixtures/${fixture}.jsonl`)).split("\n").filter(Boolean).map((l) => JSON.parse(l))
  } catch (e) {
    if (isErr(e, "NotFound")) return []
    throw e
  }
}

export async function windowOrigin(env, title) {
  // Wait for the WM to manage the window (WM_STATE) and for a stable origin
  // (see ../python/e2e.py::window_origin).
  const script =
    `wid=$(xdotool search --name "${title}" | head -1); [ -n "$wid" ] || exit 3; ` +
    `for i in $(seq 1 50); do xprop -id "$wid" WM_STATE 2>/dev/null | grep -q "window state" && break; sleep 0.1; done; ` +
    `xdotool windowactivate --sync "$wid" >/dev/null 2>&1 || true; xdotool windowraise "$wid"; ` +
    `o=; for i in $(seq 1 20); do sleep 0.3; ` +
    `n=$(xwininfo -id "$wid" | awk "/Absolute upper-left X/{x=\\$4} /Absolute upper-left Y/{y=\\$4} END{print x, y}"); ` +
    `[ "$n" = "$o" ] && break; o=$n; done; echo "$n"`
  const out = await poll(`window ${title}`, async () => {
    const o = await env.sh(`desktop-env bash -c '${script}'`, 20_000)
    return o.exit.success ? o : undefined
  }, { attempts: 30 })
  return str(out.stdout).trim().split(/\s+/).map(Number)
}

export async function clickForeground(env, x, y) {
  return JSON.parse(await env.pointerJson(JSON.stringify({ target: { delivery: "DELIVERY_FOREGROUND" }, click: { position: { x, y } } })))
}

export async function desktopChecks(env) {
  const shot = await env.screenshot(undefined)
  const [w, h] = pngSize(shot.image)
  if (w !== shot.width || h !== shot.height) throw new Error("screenshot size")
  await env.setClipboard("hello from cua-e2e ts")
  if ((await env.getClipboard()) !== "hello from cua-e2e ts") throw new Error("clipboard")
  await shOk(env, "cua-fixtures start grid")
  const [gx, gy] = await windowOrigin(env, "CUA Fixture Grid")
  await clickForeground(env, gx + 3 * 80 + 40, gy + 2 * 80 + 40)
  await poll("button_press in cell [3,2]", async () =>
    (await fixtureLog(env, "grid")).find((e) => e.type === "button_press" && e.cell?.[0] === 3 && e.cell?.[1] === 2),
    { attempts: 20, delayMs: 500 })
  await env.press("a")
  await env.hotkey(["ctrl", "b"])
  const b = await poll("key_press a and ctrl+b", async () => {
    const ev = (await fixtureLog(env, "grid")).filter((e) => e.type === "key_press")
    return ev.some((e) => e.key === "a") && ev.find((e) => e.key === "b")
  }, { attempts: 20, delayMs: 500 })
  if (!b.mods.includes("ctrl")) throw new Error(`ctrl+b mods ${JSON.stringify(b.mods)}`)
  await env.sh("cua-fixtures stop grid", 10_000).catch(() => {})
  return { screen: [shot.width, shot.height] }
}

export async function mcpInitialize(addr, token) {
  const r = await fetch(`http://${addr}/mcp`, {
    method: "POST",
    headers: { "content-type": "application/json", accept: "application/json, text/event-stream", authorization: `Bearer ${token}` },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize",
      params: { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "cua-e2e-ts", version: "0.1.0" } } }),
  })
  let text = await r.text()
  if (r.status !== 200) throw new Error(`mcp ${r.status}: ${text.slice(0, 200)}`)
  if (text.includes("data:")) text = text.split("\n").find((l) => l.startsWith("data:")).slice(5)
  const msg = JSON.parse(text)
  if (!msg.result?.serverInfo) throw new Error(`mcp: ${text.slice(0, 200)}`)
  return msg.result
}

export function parseCmdSse(body) {
  const line = str(body).split("\n").find((l) => l.startsWith("data: "))
  if (!line) throw new Error(`no data frame: ${str(body).slice(0, 200)}`)
  return JSON.parse(line.slice(6))
}

export async function legacyCmd(sb, service, command, params = {}) {
  const r = await sb.service(service).request("POST", "/cmd", u8(JSON.stringify({ command, params })).buffer, 120_000)
  if (r.status < 200 || r.status >= 300) throw new Error(`/cmd ${r.status}`)
  const p = parseCmdSse(r.body)
  if (p.success === false) throw new Error(JSON.stringify(p).slice(0, 300))
  return p
}

/**
 * `fleet.apply(name, SandboxSpec, PoolOptions)` from the flat pool fields the
 * guides list (name, image, runtime, replicas, cpu, memoryMb, services, efi,
 * command, ttlSecondsAfterCreated). Services default
 * to `{ env: 3211 }` and replicas to 1, as the deprecated `applyPool` did.
 */
export function applyPool(fleet, o) {
  let services = o.services instanceof Map ? o.services : new Map(Object.entries(o.services ?? {}))
  if (services.size === 0) services = new Map([["env", 3211]])
  const spec = cua.sandboxSpec(o.image, {
    services,
    efi: !!o.efi,
    ...(o.cpu ? { cpu: o.cpu } : {}),
    ...(o.memoryMb ? { memoryMb: o.memoryMb } : {}),
    ...(o.command?.length ? { command: o.command } : {}),
  })
  return fleet.apply(o.name, spec, cua.poolOptions({
    replicas: o.replicas ?? 1,
    ...(o.runtime ? { runtime: o.runtime } : {}),
    ...(o.ttlSecondsAfterCreated ? { poolTtlSeconds: o.ttlSecondsAfterCreated } : {}),
  }))
}

export const embeddedLocal = (dir) => cua.embedded({ stateDir: dir, fleetFromEnv: false })
export const tmpState = () => mkdtempSync(join(process.env.TMPDIR ?? "/tmp", `cua-e2e-${RUN}-state-`))
export { execFileSync }
