// `@trycua/cua/spaces` against the fixture's in-process cua-spacesd core
// (temp guest HOME/PATH, temp Downloads and teleport home, fake driver
// tools), embedded and through a `cua daemon` started from the CLI. Teleport
// ships with Cua Spaces: an embedded runtime and the MIT `cua daemon` report
// it missing, and the Cua Spaces daemon (`cua-spaces-cli daemon`) serves it,
// reading a synthetic Firefox profile through a side-effect-free host rooted
// at a temp directory. Agent runs are not started here: the fixture's driver runs
// on this host, and a real agent CLI on the host PATH must never be launched
// by a test (the Rust suites use a fake CLI in a confined PATH instead).
import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { existsSync, mkdtempSync, readFileSync, readdirSync, statSync, writeFileSync, mkdirSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { after, before, test } from "node:test"

import * as cua from "../dist/index.js"
import * as spacesApi from "../dist/spaces/index.js"
import { McpHttpTransport, McpSession, ToolError } from "../dist/spaces/transport/index.js"
import { binary, startFixtures, waitFor } from "./fixtures.mjs"

let fx
before(async () => {
  fx = await startFixtures()
})
after(async () => {
  await fx?.stop()
})

const camel = (snake) => snake.replace(/_([a-z])/g, (_, c) => c.toUpperCase())

function findFile(dir, name, budget = { n: 0 }) {
  for (const entry of readdirSync(dir)) {
    if (++budget.n > 10_000) throw new Error("bounded walk")
    const p = join(dir, entry)
    if (statSync(p).isDirectory()) {
      const hit = findFile(p, name, budget)
      if (hit) return hit
    } else if (entry === name) return p
  }
  return undefined
}

test("every contract tool maps to a generated method", () => {
  const rows = spacesApi.spacesToolMethods()
  assert.equal(rows.length, 86)
  const classes = { Spaces: spacesApi.Spaces, Space: spacesApi.Space }
  for (const row of rows) {
    const [cls, method] = row.method.split(".")
    // Reserved words get a trailing underscore (`delete` -> `delete_`).
    const proto = classes[cls].prototype
    const fn = proto[camel(method)] ?? proto[`${camel(method)}_`]
    assert.equal(typeof fn, "function", `${row.tool} -> ${row.method}`)
  }
})

async function exercise(c, tmp, teleport = false) {
  const spaces = c.spaces()
  const info = await spaces.add(fx.spaces_url, fx.spaces_token, "ts-space")
  assert.equal(info.provider, "direct")
  assert.ok(info.id.startsWith("direct:"), info.id)
  assert.ok(info.features.includes("driver"))
  assert.deepEqual((await spaces.list()).map((s) => s.id), [info.id])
  assert.equal((await spaces.resolve("ts-space")).id, info.id)
  await assert.rejects(spaces.add(fx.spaces_url, "wrong", undefined), (e) =>
    spacesApi.toSpacesError(e).code === "auth",
  )

  const space = await spaces.space(info.id)
  const out = await space.bash("echo hi; exit 3", undefined)
  assert.equal(out.stdout, "hi\n")
  assert.equal(out.exitCode, 3)
  assert.equal(out.rendered, "hi\n[exit 3]")
  assert.equal(await space.home(), fx.spaces_guest_home)

  const guest = join(fx.spaces_guest_home, "ts", "note.txt")
  const written = await space.write(guest, new TextEncoder().encode("from node").buffer)
  assert.equal(written.bytes, 9n)
  const drop = join(tmp, "drop.txt")
  writeFileSync(drop, "dropped")
  const sent = await space.sendFile(drop, spacesApi.SpaceSendFileOptions.create({ targetDirectory: "ts-inbox" }))
  assert.ok(sent.verified)
  assert.equal(readFileSync(join(fx.spaces_downloads, "ts-inbox", "drop.txt"), "utf8"), "dropped")
  const back = join(tmp, "back")
  mkdirSync(back)
  const down = await space.download(guest, back)
  assert.ok(down.verified)
  assert.equal(readFileSync(join(back, "note.txt"), "utf8"), "from node")

  const tools = await space.listTools(undefined)
  assert.ok(tools.some((t) => t.name === "get_screen_size"))
  const r = await space.callTool("get_screen_size", "{}", undefined, undefined)
  assert.equal(r.isError, false)
  assert.equal(r.text, "1280x800")

  // No desktop in this driver: refused up front, naming the feature.
  await assert.rejects(space.openStream(spacesApi.SpaceStreamOptions.create({})), (e) => {
    const s = spacesApi.toSpacesError(e)
    return s.code === "capability_missing" && /desktop_stream/.test(s.message)
  })

  const ok = spacesApi.approve(() => ({ include: undefined, acknowledgeSensitive: true }))
  if (teleport) {
    // The Cua Spaces daemon: the manifest, and a session only through the
    // Keyvault (the daemon never delivers a caller-approved session).
    const manifest = await space.teleportManifest("firefox", undefined)
    assert.ok(manifest.items.some((i) => i.isSensitive))
    await assert.rejects(space.teleport("firefox", undefined, spacesApi.approve(() => undefined)), (e) =>
      cua.CuaError.TeleportRefused.instanceOf(e),
    )
    await assert.rejects(space.teleport("firefox", undefined, ok), (e) => cua.CuaError.TeleportRefused.instanceOf(e))
  } else {
    // Without Cua Spaces: missing, and it says where it ships.
    await assert.rejects(
      space.teleportManifest("firefox", undefined),
      (e) => cua.CuaError.HostCapabilityMissing.instanceOf(e) && /Cua Spaces/.test(String(e.message ?? e)),
    )
    await assert.rejects(space.teleport("firefox", undefined, ok), (e) => cua.CuaError.HostCapabilityMissing.instanceOf(e))
  }

  const listed = await spaces.callToolJson("list_spaces", undefined)
  assert.equal(listed.isError, false)
  assert.ok(listed.text.includes(info.id))
  await assert.rejects(spacesApi.adoptThread(spaces, info.id, "run-does-not-exist"), (e) =>
    spacesApi.isSpacesError(e, "not_found") && /no run run-does-not-exist/.test(e.message),
  )
  return { spaces, info }
}

async function cleanup(spaces, info) {
  assert.ok((await spaces.delete_(info.id)).includes(info.id))
  assert.deepEqual(await spaces.list(), [])
}

test("spaces embedded", { skip: !binary("CUA_TEST_FIXTURES", "cua-test-fixtures") }, async () => {
  const dir = mkdtempSync(join(tmpdir(), "cua-ts-spaces-"))
  const c = cua.embedded({
    stateDir: join(dir, "sbx"),
    fleetFromEnv: false,
    spacesHome: join(dir, "cua"),
    teleportHome: fx.teleport_host_home,
  })
  const { spaces, info } = await exercise(c, dir)
  await cleanup(spaces, info)
})

/** Spaces through a `cua daemon` started from `cli`, and its /mcp from a
 * webview-style client. `teleport`: the Cua Spaces daemon, which serves it. */
async function daemonExercise(cli, teleport) {
    // Short path: macOS limits Unix socket paths to 104 bytes.
    const home = mkdtempSync("/tmp/cua-tss-")
    const sock = join(home, "cua.sock")
    const daemon = spawn(
      cli,
      ["daemon", "start", "--foreground", "--socket", sock, "--state-dir", join(home, "sbx")],
      {
        env: {
          ...process.env,
          CUA_HOME: home,
          HOME: home,
          CUA_SPACES_TELEPORT_HOME: fx.teleport_host_home,
          CUA_SPACES_AGENT_CREDENTIALS_HOME: "none",
        },
        stdio: "ignore",
      },
    )
    try {
      await waitFor("daemon socket", () => existsSync(sock) && existsSync(join(home, "daemon.json")), 15_000)
      const c = cua.connect(sock)
      const { spaces, info } = await exercise(c, mkdtempSync(join(tmpdir(), "cua-ts-spaces-")), teleport)
      // The daemon's registry, not this process's.
      assert.ok(existsSync(join(home, "spaces.json")))

      // The webview path: MCP over the daemon's loopback /mcp, same tools,
      // same registry.
      const discovery = JSON.parse(readFileSync(join(home, "daemon.json"), "utf8"))
      const session = new McpSession(new McpHttpTransport({ url: discovery.loopback_url, token: discovery.token }))
      const tools = await session.listTools()
      assert.equal(tools.length, 86)
      const rows = await session.callJson("list_spaces")
      assert.deepEqual(rows.map((r) => r.id), [info.id])
      assert.equal(await session.callText("space_bash", { space: info.id, command: "echo webview" }), "webview\n[exit 0]")
      await assert.rejects(session.call("stream_endpoint", { space: info.id }), (e) => e instanceof ToolError && e.kind === "capability_missing")
      await session.close()
      const refused = new McpSession(new McpHttpTransport({ url: discovery.loopback_url, token: "wrong" }))
      await assert.rejects(refused.listTools(), (e) => e.code === 401)

      await cleanup(spaces, info)
      await c.shutdownDaemon()
      await new Promise((r) => daemon.once("exit", r))
    } finally {
      if (daemon.exitCode === null) daemon.kill()
    }
}

test(
  "spaces through a cua daemon, and its /mcp from a webview-style client",
  {
    skip:
      process.platform === "win32" ||
      !binary("CUA_CLI", "cua") ||
      !binary("CUA_TEST_FIXTURES", "cua-test-fixtures"),
  },
  () => daemonExercise(binary("CUA_CLI", "cua"), false),
)

test(
  "spaces through the Cua Spaces daemon: teleport through the Keyvault",
  {
    skip:
      process.platform === "win32" ||
      !binary("CUA_SPACES_CLI", "cua-spaces-cli") ||
      !binary("CUA_TEST_FIXTURES", "cua-test-fixtures"),
  },
  () => daemonExercise(binary("CUA_SPACES_CLI", "cua-spaces-cli"), true),
)
