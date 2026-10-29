// Smoke tests of the generated Node binding against loopback fixtures, in
// the embedded topology and against a `cua daemon` started from the CLI.
import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { existsSync, mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { after, before, test } from "node:test"

import * as cua from "../dist/index.js"
import { binary, startFixtures, waitFor } from "./fixtures.mjs"

let fx
before(async () => {
  fx = await startFixtures()
})
after(async () => {
  await fx?.stop()
})

const cmd = (program, args = [], extra = {}) =>
  cua.SpacesdCommand.create({ program, args, ...extra })

// Acting as another guest user, and extended-attribute metadata (the fixture
// guest knows alice and bob, and refuses `locked`).
async function fileAccess(env) {
  const bytes = (s) => new TextEncoder().encode(s).buffer
  await env.upload(
    "/tmp/ts/as.bin",
    bytes("as alice"),
    cua.UploadOptions.create({
      user: "alice",
      metadata: new Map([["origin", "ts"]]),
      xattrNamespace: "user.e2b.",
    }),
  )
  const st = await env.stat("/tmp/ts/as.bin", "alice", "user.e2b.", false)
  assert.equal(st.owner, "alice")
  assert.equal(st.metadata.get("origin"), "ts")
  assert.equal((await env.stat("/tmp/ts/as.bin", undefined, undefined, false)).metadata.size, 0)
  const rows = await env.listDir("/tmp/ts", 1, "alice", "user.e2b.")
  assert.equal(rows.find((r) => r.name === "as.bin").metadata.get("origin"), "ts")
  assert.equal(new TextDecoder().decode(await env.download("/tmp/ts/as.bin", "alice")), "as alice")
  const moved = await env.movePath("/tmp/ts/as.bin", "/tmp/ts/as2.bin", false, true, "alice")
  assert.equal(moved.path, "/tmp/ts/as2.bin")
  await env.remove("/tmp/ts/as2.bin", false, "alice")
  assert.equal((await env.makeDir("/tmp/ts/dir", "bob")).owner, "bob")
  assert.match(
    await env.callJson("FilesystemService/Stat", '{"path":"/tmp/ts/blob"}', "bob"),
    /"entry"/,
  )
  await assert.rejects(
    env.stat("/tmp/ts/blob", "no-such", undefined, false),
    (e) => cua.CuaError.Env.instanceOf(e) && /no-such/.test(e.message),
  )
  await assert.rejects(env.stat("/tmp/ts/blob", "locked", undefined, false), (e) =>
    cua.CuaError.PermissionDenied.instanceOf(e),
  )
  await assert.rejects(env.callJson("SystemService/Health", "{}", "alice"), (e) =>
    cua.CuaError.InvalidArgument.instanceOf(e),
  )
}

async function exercise(c) {
  const sbx = c.sandboxes()
  const sb = await sbx.connectUrl(fx.env_url, fx.env_token, "ts-direct")
  assert.equal(sb.name(), "ts-direct")
  assert.equal(sb.location(), "direct")
  const env = await sb.spacesd(5000)

  const caps = await env.capabilities()
  assert.ok(caps.version.length > 0)
  // How the guest agent runs commands and file operations as another user:
  // itself, with no sudo ("root" or "capabilities"), through sudo (older
  // images) or not at all ("none").
  const runAs = caps.features.find((f) => f.name === "process.run_as")
  assert.ok(runAs.supported)
  assert.equal(runAs.attributes.get("mode"), "capabilities")
  const asAlice = await env.run(cmd("echo", ["as alice"], { user: "alice" }))
  assert.ok(asAlice.exit.success)
  assert.equal(new TextDecoder().decode(asAlice.stdout), "as alice\n")
  const out = await env.run(cmd("echo", ["hi"]))
  assert.ok(out.exit.success)
  assert.equal(new TextDecoder().decode(out.stdout), "hi\n")

  const p = await env.spawn(cmd("cat", [], { stdin: true }))
  await p.writeStdin(new TextEncoder().encode("xyz").buffer)
  await p.closeStdin()
  assert.equal(new TextDecoder().decode((await p.wait()).stdout), "xyz")

  const blob = new Uint8Array(200_000).map((_, i) => i % 251)
  const up = await env.upload("/tmp/ts/blob", blob.buffer, undefined)
  assert.equal(up.size, BigInt(blob.length))
  const down = new Uint8Array(await env.download("/tmp/ts/blob", undefined))
  assert.deepEqual(down, blob)

  await fileAccess(env)

  await env.setClipboard("from node")
  assert.equal(await env.getClipboard(), "from node")
  assert.match(await env.callJson("SystemService/Health", "{}", undefined), /^\{/)

  await assert.rejects(env.download("/nope", undefined), (e) => cua.CuaError.NotFound.instanceOf(e))
  await assert.rejects(
    env.callJson("ProcessService/StartProcess", "{}", undefined),
    (e) => cua.CuaError.InvalidArgument.instanceOf(e),
  )

  const frames = []
  const events = []
  const audio = []
  const sink = {
    onFrame: (f) => frames.push(f),
    onEvent: (e) => events.push(e),
  }
  const session = await env.openMediaWithAudio(
    cua.MediaOpenOptions.create({ audio: true }),
    sink,
    { onAudio: (a) => audio.push(a) },
  )
  assert.equal(session.codec(), "h264")
  await waitFor("frames", () => frames.length >= 2 && audio.length >= 1)
  assert.equal(frames[0].keyframe, true)
  assert.equal(frames[0].sequence, 7n)
  assert.deepEqual(events.slice(0, 2).map((e) => e.kind), ["hello", "session_opened"])
  await session.close()

  const bad = await sbx.connectUrl(fx.env_url, "wrong", "ts-bad")
  await assert.rejects(bad.spacesd(3000), (e) => cua.CuaError.Unauthenticated.instanceOf(e))
  // One machine, one ref (`direct:<host:port>`): the latest connection to an
  // address is the one the ref reaches. Reconnect with the right token.
  assert.equal(bad.id(), sb.id())
  assert.ok(sb.id().startsWith("direct:127.0.0.1:"), sb.id())
  await sbx.connectUrl(fx.env_url, fx.env_token, "ts-direct")

  const listing = await sbx.listWithWarnings(undefined)
  // A machine cua only connects to has no shape of its own to change.
  await assert.rejects(sb.resize(2, 2048n, undefined), (e) => cua.CuaError.Unsupported.instanceOf(e))
  assert.ok(listing.sandboxes.some((s) => s.name === "ts-direct" && s.location === "direct"))
  assert.deepEqual(listing.warnings, [])
  // The default lists the sandboxes on this machine.
  assert.ok(!(await sbx.list("local")).some((s) => s.name === "ts-direct"))
  await sb.delete_()
  await assert.rejects(sbx.get("nope"), (e) => cua.CuaError.NotFound.instanceOf(e))

  const report = await c.local().doctor()
  assert.ok(report.checks.some((ch) => ch.name === "qemu"))
}

test("embedded", { skip: !binary("CUA_TEST_FIXTURES", "cua-test-fixtures") }, async () => {
  const dir = mkdtempSync(join(tmpdir(), "cua-ts-"))
  const c = cua.embedded({ stateDir: join(dir, "sbx") })
  assert.equal(c.mode(), cua.CuaMode.Embedded)
  await exercise(c)
})

// Cua Cloud has ended: every `on: "cloud"` create (and a `pool:<name>` image)
// fails with the one "Cua Cloud has ended" message.
const cloudEnded = (e) => {
  assert.ok(cua.CuaError.Unsupported.instanceOf(e), String(e))
  assert.match(e.message, /Cua Cloud has ended/)
  return true
}

test("cloud creates fail with the cloud-ended error", () => {
  const dir = mkdtempSync(join(tmpdir(), "cua-ts-"))
  const c = cua.embedded({ stateDir: join(dir, "sandboxes") })
  // A digest-pinned image: nothing resolves tags against a registry.
  const image = "ghcr.io/trycua/cua-e2e-ts@sha256:0123"
  const sbx = c.sandboxes()
  return Promise.all([
    assert.rejects(
      sbx.create(cua.SandboxCreateOptions.create({ on: "cloud", image, kind: "vm" })),
      cloudEnded,
    ),
    assert.rejects(sbx.create(cua.SandboxCreateOptions.create({ image: "pool:a-pool" })), cloudEnded),
  ]).then(async () => {
    // Nothing was created, and cloud is not a listed location.
    assert.deepEqual((await sbx.listWithWarnings("cloud")).sandboxes, [])
    assert.ok(!cua.sandboxLocations().includes("cloud"))
  })
})
