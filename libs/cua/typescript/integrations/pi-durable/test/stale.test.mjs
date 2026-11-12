// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// A driver whose epoch is stale halts: the refusal comes up through the
// wrapped tool as StaleEpochError and is never recorded as the call's outcome;
// live output stops being forwarded once the fence says so.

import assert from "node:assert/strict"
import { mkdirSync, mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { createBashTool } from "@earendil-works/pi-durable/tools"
import { durableSpaces, openFencedSqlite, StaleEpochError } from "../dist/index.js"
import { CountingSnapshots, LocalSpacesd, localSpaces } from "./local-space.mjs"
import { context, fakeApi, wrapped } from "./wrapped.mjs"

async function setup(epoch, { mode = "transactional", withFence = true, dir } = {}) {
  dir ??= mkdtempSync(join(tmpdir(), "pdv2-stale-"))
  const cwd = join(dir, "ws")
  mkdirSync(cwd, { recursive: true })
  const spacesd = new LocalSpacesd(join(dir, "space"))
  const log = withFence ? await openFencedSqlite(join(dir, "log.db"), epoch) : undefined
  const stale = []
  const durable = durableSpaces({
    spaces: localSpaces(spacesd),
    snapshots: new CountingSnapshots(),
    cwd,
    spaceName: () => "s",
    incarnationMarker: join(dir, "incarnation"),
    epoch,
    mode,
    ...(log ? { fence: log.fence } : {}),
    onStale: (e) => stale.push(e),
  })
  const bash = wrapped(durable, createBashTool())
  return { dir, cwd, spacesd, log, durable, bash, stale }
}

const call = (command) => ({ command })

for (const mode of ["transactional", "checkpoint"]) {
  test(`${mode}: a Space that moved to a newer epoch refuses the command; the driver gets StaleEpochError and nothing is recorded`, async () => {
    const t = await setup(3, { mode, withFence: false })
    const api = fakeApi()
    const first = await t.bash.execute(call("echo one > a.txt"), api, context)
    assert.equal(first.isError ?? false, false)
    // A newer driver takes the Space over.
    await t.spacesd.callJson("ProcessService/AdvanceEpoch", JSON.stringify({ epoch: "4" }))
    const second = fakeApi({ taskId: 101, callId: "call-2" })
    // The first doc state of a second call is empty; share the conversation's docs.
    for (const [k, v] of api.docs) if (!k.endsWith(":100")) second.docs.set(k, structuredClone(v))
    await assert.rejects(
      t.bash.execute(call("echo two > b.txt"), second, context),
      (error) => error instanceof StaleEpochError && error.epoch === 3 && error.floor === 4 && error.resource === "space",
    )
    assert.equal(second.docs.get("cua.space.call:101")?.outcome, undefined, "no outcome was recorded")
    assert.equal(second.outputs.length, 0)
    assert.equal(t.stale.length, 1)
    assert.ok(t.stale[0] instanceof StaleEpochError)
    await t.durable.close()
  })
}

test("a driver below the Space's epoch is stale at first use, before any command", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pdv2-stale-"))
  const newer = await setup(5, { dir, withFence: false })
  await newer.bash.execute(call("true"), fakeApi(), context)
  const older = await setup(2, { dir, withFence: false })
  const before = newer.spacesd.spawned.length
  await assert.rejects(older.bash.execute(call("echo late > late.txt"), fakeApi(), context), StaleEpochError)
  assert.equal(newer.spacesd.spawned.length, before, "the stale driver spawned nothing that ran a command")
  await newer.durable.close()
  await older.durable.close()
})

test("a log whose floor is above the driver's epoch stops the call before it touches the Space", async () => {
  const t = await setup(1)
  await openFencedSqlite(join(t.dir, "log.db"), 2)
  const spawned = t.spacesd.spawned.length
  await assert.rejects(t.bash.execute(call("echo x > x.txt"), fakeApi(), context), StaleEpochError)
  assert.equal(t.spacesd.spawned.length, spawned)
  assert.equal(t.stale.length, 1)
  await t.durable.close()
})

test("live output is not forwarded once the fence has seen a newer epoch, and the call ends in StaleEpochError", async () => {
  const t = await setup(1)
  const fake = {
    name: "bash",
    replay: "unsafe",
    async execute(_args, api) {
      api.output("before\n")
      await openFencedSqlite(join(t.dir, "log.db"), 2) // the newer driver opens the log
      await t.log.fence.check().catch(() => {}) // the next periodic check sees it
      api.output("after\n")
      return { content: [{ type: "text", text: "ok" }] }
    },
  }
  const api = fakeApi()
  const tool = wrapped(t.durable, fake)
  await assert.rejects(tool.execute({}, api, context), StaleEpochError)
  assert.deepEqual(api.outputs, ["before\n"])
  assert.equal(api.docs.get("cua.space.call:100")?.outcome, undefined)
  await t.durable.close()
})
