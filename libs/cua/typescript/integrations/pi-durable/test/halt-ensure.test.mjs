// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// An epoch operation that fails for a reason other than staleness halts the
// driver instead of becoming a tool result; ensureSpace rebuilds a Space lost
// after the last call.

import assert from "node:assert/strict"
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { createBashTool } from "@earendil-works/pi-durable/tools"
import { advanceEpoch, DirectoryCheckpointStore, durableSpaces, getExecRecord, EXIT_CRASHED, EXIT_PERMANENT, HaltError, openFencedSqlite, runDurableSession, WorkspaceSnapshots } from "../dist/index.js"
import { context, open } from "./fixture.mjs"
import { CountingSnapshots, LocalSpacesd, localSpaces, workspaceListing } from "./local-space.mjs"
import { fakeApi, wrapped } from "./wrapped.mjs"

function space() {
  const dir = mkdtempSync(join(tmpdir(), "pdv2-halt-"))
  const cwd = join(dir, "ws")
  mkdirSync(cwd)
  return { dir, cwd, spacesd: new LocalSpacesd(join(dir, "space")) }
}

/** A Space whose daemon fails AdvanceEpoch with `message`. */
function failing(spacesd, message) {
  return new Proxy(spacesd, {
    get(target, property) {
      if (property === "callJson") {
        return async (method, json) => {
          if (method === "ProcessService/AdvanceEpoch") throw new Error(message)
          return target.callJson(method, json)
        }
      }
      const value = Reflect.get(target, property, target)
      return typeof value === "function" ? value.bind(target) : value
    },
  })
}

const UNSUPPORTED = '"ProcessService/AdvanceEpoch" is not a JSON-callable cua.env.v1 method'

for (const epoch of [undefined, 3]) {
  test(`an SDK without AdvanceEpoch halts the driver with a clear permanent error (${epoch ? "session epoch" : "no session epoch"}); nothing is recorded`, async () => {
    const { dir, cwd, spacesd } = space()
    const halts = []
    const durable = durableSpaces({
      spaces: localSpaces(failing(spacesd, UNSUPPORTED)),
      snapshots: new CountingSnapshots(),
      cwd,
      spaceName: () => "s",
      incarnationMarker: join(dir, "incarnation"),
      ...(epoch ? { epoch } : {}),
      onHalt: (e) => halts.push(e),
    })
    const api = fakeApi()
    await assert.rejects(
      wrapped(durable, createBashTool()).execute({ command: "echo x > x.txt" }, api, context),
      (e) => e instanceof HaltError && e.permanent && /does not support ProcessService\/AdvanceEpoch/.test(e.message) && /needs a newer @trycua\/cua/.test(e.message),
    )
    assert.equal(halts.length, 1)
    assert.equal(api.docs.get("cua.space.call:100")?.outcome, undefined)
    assert.deepEqual(workspaceListing(cwd), {}, "the command never ran")
    await durable.close()
  })
}

test("an unexpected AdvanceEpoch failure halts as a transient error", async () => {
  const { dir, cwd, spacesd } = space()
  const durable = durableSpaces({ spaces: localSpaces(failing(spacesd, "connection reset")), snapshots: new CountingSnapshots(), cwd, spaceName: () => "s", incarnationMarker: join(dir, "i"), epoch: 2 })
  await assert.rejects(wrapped(durable, createBashTool()).execute({ command: "true" }, fakeApi(), context), (e) => e instanceof HaltError && !e.permanent && /connection reset/.test(e.message))
  await durable.close()
})

test("through the driver: exit 78, the log's fence is halted so Pi records no tool result", async () => {
  const { dir, cwd, spacesd } = space()
  const stateDir = join(dir, "state")
  const code = await runDurableSession({
    env: { CUA_SESSION: "s", CUA_SESSION_EPOCH: "1", CUA_SESSION_STATE_DIR: stateDir },
    start: async ({ log, fence, onStale, onHalt }) => {
      const durable = durableSpaces({ spaces: localSpaces(failing(spacesd, UNSUPPORTED)), snapshots: new CountingSnapshots(), cwd, spaceName: () => "s", incarnationMarker: join(dir, "i"), epoch: 1, fence, onStale, onHalt })
      const s = await open({ dir: join(dir, "unused"), cwd, spacesd, plan: [{ name: "bash", arguments: { command: "echo x > x.txt" } }], storageOverride: log.storage, durableOverride: durable })
      return { wait: () => s.run(), close: () => s.close() }
    },
  })
  assert.equal(code, EXIT_PERMANENT)
  // The log holds no tool result.
  const reopened = await openFencedSqlite(join(stateDir, "log.db"), 1)
  const { Harness } = await import("@earendil-works/pi-durable")
  const { createModels } = await import("@earendil-works/pi-ai")
  const { createRegistry } = await import("@earendil-works/pi-durable")
  const harness = await Harness.open(reopened.storage, { models: createModels(), registry: createRegistry() }, context)
  const root = await harness.root(context)
  const messages = (await root.context(context)).messages
  assert.equal(messages.filter((m) => m.role === "toolResult").length, 0)
  await harness.close(context)
  void EXIT_CRASHED
})

test("ensureSpace: a Space lost after the last call is rebuilt from the log's image (transactional)", async () => {
  const { dir, cwd, spacesd } = space()
  const plan = [{ name: "bash", arguments: { command: "echo one > a.txt; mkdir d; echo two > d/b.txt" } }]
  const first = await open({ dir, cwd, spacesd, epoch: 1, mode: "transactional", plan })
  await first.run()
  await first.close()
  const want = workspaceListing(cwd)
  rmSync(cwd, { recursive: true })
  mkdirSync(cwd)
  rmSync(join(dir, "incarnation"))
  const second = await open({ dir, cwd, spacesd, epoch: 2, mode: "transactional", plan })
  try {
    await second.run("again") // everything is done: no call touches the Space
    assert.deepEqual(workspaceListing(cwd), {})
    await second.durable.ensureSpace(second.harness, second.root.id, context)
    assert.deepEqual(workspaceListing(cwd), want)
  } finally {
    await second.close()
  }
})

test("ensureSpace: a Space lost after the last call is rebuilt from the newest checkpoint (checkpoint mode)", async () => {
  const { dir, cwd, spacesd } = space()
  const spaces = localSpaces(spacesd)
  const snapshots = () => new WorkspaceSnapshots({ spaces, root: cwd, guestDir: join(dir, "guest-store"), store: new DirectoryCheckpointStore(join(dir, "host-store")) })
  const plan = [{ name: "bash", arguments: { command: "echo one > a.txt; mkdir d; echo two > d/b.txt" } }]
  const first = await open({ dir, cwd, spacesd, epoch: 1, mode: "checkpoint", plan, snapshots: snapshots() })
  await first.run()
  await first.close()
  const want = workspaceListing(cwd)
  assert.ok(want["a.txt"])
  rmSync(cwd, { recursive: true })
  rmSync(join(dir, "guest-store"), { recursive: true, force: true })
  rmSync(join(dir, "incarnation"))
  mkdirSync(cwd)
  const second = await open({ dir, cwd, spacesd, epoch: 2, mode: "checkpoint", plan, snapshots: snapshots() })
  try {
    // Nothing has touched the Space yet.
    assert.deepEqual(workspaceListing(cwd), {})
    await second.durable.ensureSpace(second.harness, second.root.id, context)
    assert.deepEqual(workspaceListing(cwd), want)
  } finally {
    await second.close()
  }
})

test("checkpoint mode, a replaced Space and checkpoints kept only inside the lost Space: the driver stops for good with the reason", async () => {
  const { dir, cwd, spacesd } = space()
  const spaces = localSpaces(spacesd)
  // No CheckpointStore: the checkpoints live in the Space (guest-store) and go with it.
  const snapshots = () => new WorkspaceSnapshots({ spaces, root: cwd, guestDir: join(dir, "guest-store") })
  const plan = [{ name: "bash", arguments: { command: "echo one > a.txt" } }]
  const first = await open({ dir, cwd, spacesd, epoch: 1, mode: "checkpoint", plan, snapshots: snapshots() })
  await first.run()
  await first.close()
  rmSync(cwd, { recursive: true })
  rmSync(join(dir, "guest-store"), { recursive: true, force: true })
  rmSync(join(dir, "incarnation"))
  mkdirSync(cwd)
  const halts = []
  const second = await open({ dir, cwd, spacesd, epoch: 2, mode: "checkpoint", plan, snapshots: snapshots(), onHalt: (e) => halts.push(e) })
  try {
    await assert.rejects(second.durable.ensureSpace(second.harness, second.root.id, context).then(() => second.run()), (e) => e instanceof HaltError && e.permanent && /use a CheckpointStore or mode "transactional"/.test(e.message))
    assert.equal(halts.length, 1)
    assert.equal(halts[0].permanent, true)
  } finally {
    await second.close()
  }
})

test("a fresh log whose session epoch equals the Space's epoch (another log's restore bump) runs above it", async () => {
  const { dir, cwd, spacesd } = space()
  // An earlier session on this durable Space ended at epoch 3 although its session epoch was 2.
  await advanceEpoch(spacesd, 3)
  const durable = durableSpaces({ spaces: localSpaces(spacesd), snapshots: new CountingSnapshots(), cwd, spaceName: () => "s", incarnationMarker: join(dir, "i"), epoch: 3 })
  await wrapped(durable, createBashTool()).execute({ command: "echo x > x.txt" }, fakeApi(), context)
  assert.equal((await getExecRecord(spacesd, "probe")).epoch, 4, "the Space moved to 4: no key of the other log can match")
  await durable.close()
})

test("a session epoch above the Space's epoch is used as is, and a stale one is still refused", async () => {
  const { dir, cwd, spacesd } = space()
  await advanceEpoch(spacesd, 2)
  const fresh = durableSpaces({ spaces: localSpaces(spacesd), snapshots: new CountingSnapshots(), cwd, spaceName: () => "s", incarnationMarker: join(dir, "i"), epoch: 5 })
  await wrapped(fresh, createBashTool()).execute({ command: "true" }, fakeApi(), context)
  assert.equal((await getExecRecord(spacesd, "probe")).epoch, 5)
  await fresh.close()
  const stale = durableSpaces({ spaces: localSpaces(spacesd), snapshots: new CountingSnapshots(), cwd, spaceName: () => "s", incarnationMarker: join(dir, "i2"), epoch: 4, onStale: () => {} })
  await assert.rejects(wrapped(stale, createBashTool()).execute({ command: "true" }, fakeApi(), context), (e) => /stale|newer|older/i.test(String(e?.message)) || e?.name === "StaleEpochError")
  await stale.close()
})

/** A Space that answers until `down.on`, then fails every call as a vanished container does. */
function vanishing(spacesd, down) {
  const dead = () => {
    throw new Error("CuaError.Transport: transport: transport error: tcp connect error (Connection refused (os error 111))")
  }
  return new Proxy(spacesd, {
    get(target, property) {
      const value = Reflect.get(target, property, target)
      if (typeof value !== "function") return value
      return (...args) => (down.on ? dead() : value.apply(target, args))
    },
  })
}

for (const mode of ["checkpoint", "transactional"]) {
  test(`${mode}: a Space that vanishes under a call halts the driver (transient); the failure is not recorded as a tool result`, async () => {
    const { dir, cwd, spacesd } = space()
    const down = { on: false }
    const halts = []
    const durable = durableSpaces({
      spaces: localSpaces(vanishing(spacesd, down)),
      snapshots: new CountingSnapshots(),
      cwd,
      spaceName: () => "s",
      mode,
      incarnationMarker: join(dir, "i"),
      epoch: 2,
      onHalt: (e) => halts.push(e),
    })
    const api = fakeApi()
    const tool = wrapped(durable, createBashTool())
    await tool.execute({ command: "echo one > one.txt" }, api, context)
    // The container is removed while the next call is bound to it.
    down.on = true
    await assert.rejects(tool.execute({ command: "echo two > two.txt" }, fakeApi(), context), (e) => e instanceof HaltError && !e.permanent && /cannot be reached/.test(e.message))
    assert.equal(halts.length, 1)
    assert.equal(halts[0].permanent, false)
    // A command failure on a reachable Space is still an ordinary tool result.
    down.on = false
    const failing = await wrapped(durable, createBashTool()).execute({ command: "exit 3" }, fakeApi(), context).catch((e) => e)
    assert.ok(!(failing instanceof HaltError), "a failing command on a reachable Space is not a halt")
    await durable.close()
  })
}
