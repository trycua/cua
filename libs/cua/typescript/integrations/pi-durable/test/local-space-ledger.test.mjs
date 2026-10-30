// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// The cua-spacesd stand-in's ledger entry appears whole or not at all, and a started
// command records its own pid. Found by the stale mode of the crash campaign (seed 1303 of
// the CI subset, 0.5 CPU): the stand-in made the entry directory and wrote meta.json after,
// so the next worker's GetExecRecord, running while the first worker was frozen between the
// two, read an empty file ("Unexpected end of JSON input"), failed the call, and the run
// ended with a workspace the log did not imply.

import assert from "node:assert/strict"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { Worker } from "node:worker_threads"
import { LocalSpacesd } from "./local-space.mjs"

test("a concurrent GetExecRecord never sees a half-made ledger entry", async () => {
  const dir = mkdtempSync(join(tmpdir(), "ledger-"))
  try {
    const spacesd = new LocalSpacesd(join(dir, "s"))
    const ids = Array.from({ length: 300 }, (_, i) => `r${i}`)
    const reader = new Worker(
      `const { parentPort, workerData } = require("node:worker_threads")
       import("${new URL("./local-space.mjs", import.meta.url).href}").then(async ({ LocalSpacesd }) => {
         const s = new LocalSpacesd(workerData.state)
         const errors = []
         let stop = false
         parentPort.on("message", () => { stop = true })
         while (!stop) {
           for (const id of workerData.ids) {
             try { await s.callJson("ProcessService/GetExecRecord", JSON.stringify({ requestId: id })) }
             catch (e) { errors.push(String(e.message)) }
           }
           await new Promise((r) => setImmediate(r))
         }
         parentPort.postMessage(errors)
       })`,
      { eval: true, workerData: { state: join(dir, "s"), ids } },
    )
    const result = new Promise((resolve) => reader.once("message", resolve))
    for (const id of ids) await spacesd.spawn({ program: "true", args: [], cwd: dir, requestId: id, tag: id, epoch: 1 })
    reader.postMessage("stop")
    assert.deepEqual(await result, [])
    await reader.terminate()
    // Each started command recorded its own pid and exited: the ledger reports it, not "lost".
    await new Promise((r) => setTimeout(r, 1500))
    const record = JSON.parse(await spacesd.callJson("ProcessService/GetExecRecord", JSON.stringify({ requestId: "r0" })))
    assert.equal(record.record.state, "EXEC_STATE_EXITED")
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("an entry with an empty meta.json is an unclaimed key, not a failed call", async () => {
  const dir = mkdtempSync(join(tmpdir(), "ledger-"))
  try {
    const spacesd = new LocalSpacesd(join(dir, "s"))
    const entry = spacesd.stateDir + "/ledger"
    const probe = JSON.parse(await spacesd.callJson("ProcessService/GetExecRecord", JSON.stringify({ requestId: "x" })))
    assert.equal(probe.record, undefined)
    // A client frozen between the mkdir and the write of the old stand-in left exactly this.
    const { createHash } = await import("node:crypto")
    const { mkdirSync, writeFileSync } = await import("node:fs")
    const name = createHash("sha256").update("y").digest("hex").slice(0, 24)
    mkdirSync(join(entry, name))
    writeFileSync(join(entry, name, "meta.json"), "")
    const torn = JSON.parse(await spacesd.callJson("ProcessService/GetExecRecord", JSON.stringify({ requestId: "y" })))
    assert.equal(torn.record, undefined)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

// Campaign seed 1101 (auto mode, CI subset, 0.5 CPU): the worker was killed right after it asked
// the stand-in to start `echo s1 >> log.txt`. The command had been forked but had not yet written
// its pid, so the next worker saw no live process, undid the call and fenced the Space with
// AdvanceEpoch, which only kills commands that have a pid. The starting command then ran after the
// undo, and the re-run appended a second line. The real daemon starts and fences in one step; the
// stand-in now checks the epoch after the pid is visible. This builds the interleaving exactly:
// the start is held before the pid, the epoch is advanced, then the start is released.
test("a command still starting when the epoch advances does not run", async () => {
  const dir = mkdtempSync(join(tmpdir(), "ledger-"))
  try {
    const { existsSync, readdirSync, writeFileSync } = await import("node:fs")
    const hold = join(dir, "hold")
    writeFileSync(hold, "")
    const marker = join(dir, "ran")
    const spacesd = new LocalSpacesd(join(dir, "s"), { holdStart: hold })
    await spacesd.spawn({ requestId: "call-1", epoch: 1, program: "/bin/sh", args: ["-c", `echo ran >> ${marker}`], cwd: dir })
    // The old driver is gone; the new one fences the Space while the command is still starting.
    const advanced = JSON.parse(await spacesd.callJson("ProcessService/AdvanceEpoch", JSON.stringify({ epoch: "2" })))
    assert.equal(advanced.killed, 0, "no pid yet: nothing for the advance to kill")
    rmSync(hold)
    // Cut off by the epoch like a killed command: it ends without an exit code, so its fate is unknown.
    const entry = join(spacesd.stateDir, "ledger", readdirSync(join(spacesd.stateDir, "ledger"))[0])
    for (let i = 0; i < 3000 && !existsSync(join(entry, "aborted")) && !existsSync(marker); i++) await new Promise((r) => setTimeout(r, 10))
    assert.equal(existsSync(join(entry, "aborted")), true, "the command noticed the new epoch before it ran")
    const record = JSON.parse(await spacesd.callJson("ProcessService/GetExecRecord", JSON.stringify({ requestId: "call-1" }))).record
    assert.equal(record?.state, "EXEC_STATE_LOST")
    assert.equal(existsSync(marker), false, "the stale command must not run after the advance")
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

// Campaign seed 1309 (stale mode, found while validating the fix above): the old driver passed the
// stand-in's epoch check, was frozen, and claimed its key after the next driver had advanced the
// epoch; the next driver then asked for the same key and was attached to the dead claim (its
// command had been cut off), so its own call came back as "outcome unknown". The real daemon
// refuses a stale epoch and claims in one step, so there the key is simply free.
test("a key claimed late by a fenced-out client is free for the current epoch", async () => {
  const dir = mkdtempSync(join(tmpdir(), "ledger-"))
  try {
    const { existsSync } = await import("node:fs")
    const spacesd = new LocalSpacesd(join(dir, "s"))
    await spacesd.callJson("ProcessService/AdvanceEpoch", JSON.stringify({ epoch: "2" }))
    // The stale client's claim lands after the advance (its epoch check passed before it): this
    // client still sees the Space at epoch 1 when it checks.
    const late = new LocalSpacesd(join(dir, "s"))
    late.epoch = () => 1
    await late.spawn({ requestId: "k", epoch: 1, program: "/bin/sh", args: ["-c", "echo stale >> " + join(dir, "out")], cwd: dir })
    // Let the stale command abort itself.
    await new Promise((r) => setTimeout(r, 300))
    const fresh = new LocalSpacesd(join(dir, "s"))
    const process_ = await fresh.spawn({ requestId: "k", epoch: 2, program: "/bin/sh", args: ["-c", "echo fresh"], cwd: dir })
    let event
    const seen = []
    while ((event = await process_.nextEvent())) {
      if (event.kind === 0) seen.push(Buffer.from(event.data).toString())
      if (event.kind === 3) {
        assert.equal(event.exit.success, true, JSON.stringify(event.exit))
        break
      }
    }
    assert.deepEqual(seen, ["fresh\n"])
    assert.equal(existsSync(join(dir, "out")), false, "the stale command never ran")
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
