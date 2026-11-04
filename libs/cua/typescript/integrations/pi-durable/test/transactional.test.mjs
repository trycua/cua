// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// Transactional mode: the workspace scan, diff and reconcile against a local
// Space, and the mode a durable session picks and what it commits.

import assert from "node:assert/strict"
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { createBashTool } from "@earendil-works/pi-durable/tools"
import { ImageDoc, SpaceDoc } from "../dist/index.js"
import {
  applyDiff,
  capture,
  diff,
  reconcile,
  scan,
  DEFAULT_TRANSACTIONAL_LIMITS as limits,
  TransactionalLimitExceeded,
  TransactionalUnsupported,
} from "../dist/transactional.js"
import { context, open } from "./fixture.mjs"
import { CountingSnapshots, LocalSpacesd, workspaceListing } from "./local-space.mjs"

function space() {
  const dir = mkdtempSync(join(tmpdir(), "pdv2-tx-"))
  const cwd = join(dir, "ws")
  mkdirSync(cwd)
  return { dir, cwd, spacesd: new LocalSpacesd(join(dir, "space")) }
}

const write = (cwd, path, content, mode) => {
  mkdirSync(join(cwd, path, ".."), { recursive: true })
  writeFileSync(join(cwd, path), content)
  chmodSync(join(cwd, path), mode ?? 0o644) // explicit: the test asserts modes, and a 002 umask (Ubuntu) would give 0o664
}

/** The image a manifest of `cwd` makes, as the log would hold it. */
async function imageOf(spacesd, cwd, lim = limits) {
  const manifest = await scan(spacesd, cwd, lim)
  const image = {}
  applyDiff(image, manifest, diff(undefined, manifest), await capture(spacesd, cwd, manifest, Object.keys(manifest), new Set()))
  return { image, manifest }
}

test("scan lists files, links, modes and hashes, and honors exclusions", async () => {
  const { cwd, spacesd } = space()
  write(cwd, "a.txt", "alpha\n")
  write(cwd, "bin/run.sh", "#!/bin/sh\n", 0o755)
  write(cwd, "skip/big.bin", "x".repeat(100))
  write(cwd, "with space/it's.txt", "q")
  symlinkSync("a.txt", join(cwd, "link"))
  const all = await scan(spacesd, cwd, limits)
  assert.deepEqual(Object.keys(all).sort(), ["a.txt", "bin/run.sh", "link", "skip/big.bin", "with space/it's.txt"])
  assert.equal(all["bin/run.sh"].m, 0o755)
  assert.equal(all["a.txt"].m, 0o644)
  assert.equal(all["a.txt"].s, 6)
  assert.equal(all.link.k, "l")
  assert.equal(all.link.t, "a.txt")
  const some = await scan(spacesd, cwd, { ...limits, exclude: ["skip"] })
  assert.ok(!("skip/big.bin" in some))
  assert.deepEqual(await scan(spacesd, join(cwd, "bin"), limits).then(Object.keys), ["run.sh"])
})

test("scan refuses a workspace over the file or byte limit, and names it cannot represent", async () => {
  const { cwd, spacesd } = space()
  for (let i = 0; i < 12; i++) write(cwd, `f${i}.txt`, "x".repeat(10))
  await assert.rejects(
    scan(spacesd, cwd, { ...limits, maxFiles: 10 }),
    (e) => e instanceof TransactionalLimitExceeded && e.reason === "files",
  )
  await assert.rejects(
    scan(spacesd, cwd, { ...limits, maxBytes: 100 }),
    (e) => e instanceof TransactionalLimitExceeded && e.reason === "bytes",
  )
  assert.equal(Object.keys(await scan(spacesd, cwd, { ...limits, maxFiles: 12 })).length, 12)
  write(cwd, "bad\\name.txt", "x")
  await assert.rejects(scan(spacesd, cwd, limits), TransactionalUnsupported)
  rmSync(join(cwd, "bad\\name.txt"))
  write(cwd, "bad\nname.txt", "x")
  await assert.rejects(scan(spacesd, cwd, limits), TransactionalUnsupported)
})

test("diff names what changed; applyDiff keeps only the blobs the image uses", async () => {
  const { cwd, spacesd } = space()
  write(cwd, "a.txt", "one")
  write(cwd, "b.txt", "two")
  const { image, manifest } = await imageOf(spacesd, cwd)
  assert.deepEqual(diff(image.files, manifest), { upsert: [], remove: [] })
  write(cwd, "a.txt", "ONE")
  rmSync(join(cwd, "b.txt"))
  write(cwd, "c.txt", "two")
  chmodSync(join(cwd, "c.txt"), 0o600)
  const next = await scan(spacesd, cwd, limits)
  const changes = diff(image.files, next)
  assert.deepEqual(changes, { upsert: ["a.txt", "c.txt"], remove: ["b.txt"] })
  // c.txt has b.txt's old content: the blob is already in the image and is not read again.
  const blobs = await capture(spacesd, cwd, next, changes.upsert, new Set(Object.keys(image.blobs)))
  assert.equal(blobs.size, 1)
  applyDiff(image, next, changes, blobs)
  assert.deepEqual(Object.keys(image.files).sort(), ["a.txt", "c.txt"])
  assert.equal(Object.keys(image.blobs).length, 2, "the old a.txt blob is gone, b/c's shared blob stays")
})

test("reconcile makes the workspace equal the image: restores, removes, repairs modes, links and directory conflicts", async () => {
  const { cwd, spacesd } = space()
  write(cwd, "keep.txt", "keep")
  write(cwd, "edit.txt", "before")
  write(cwd, "deep/er/file.txt", "deep")
  write(cwd, "run.sh", "#!/bin/sh", 0o755)
  symlinkSync("keep.txt", join(cwd, "ln"))
  const { image } = await imageOf(spacesd, cwd)
  const want = workspaceListing(cwd)
  // Damage it every way a crash can.
  write(cwd, "edit.txt", "half written")
  rmSync(join(cwd, "keep.txt"))
  rmSync(join(cwd, "deep"), { recursive: true })
  write(cwd, "extra.txt", "x")
  write(cwd, "newdir/extra2.txt", "y")
  chmodSync(join(cwd, "run.sh"), 0o644)
  // unlinkSync: rmSync leaves a dangling symlink in place on some Node 24 releases (24.9 on Linux).
  unlinkSync(join(cwd, "ln"))
  mkdirSync(join(cwd, "ln"))
  write(cwd, "ln/inside", "z")
  const done = await reconcile(spacesd, cwd, image, limits)
  assert.ok(done.written >= 5 && done.deleted >= 3, JSON.stringify(done))
  assert.deepEqual(workspaceListing(cwd), want)
  const after = await scan(spacesd, cwd, limits)
  assert.equal(after["run.sh"].m, 0o755)
  assert.equal(after.ln.k, "l")
  // Idempotent.
  assert.deepEqual(await reconcile(spacesd, cwd, image, limits), { written: 0, deleted: 0 })
})

test("reconcile on an empty Space rebuilds the whole workspace from the log", async () => {
  const { cwd, spacesd } = space()
  for (let i = 0; i < 30; i++) write(cwd, `d${i % 3}/f${i}.txt`, `content ${i}`)
  const { image } = await imageOf(spacesd, cwd)
  const want = workspaceListing(cwd)
  rmSync(cwd, { recursive: true })
  mkdirSync(cwd)
  await reconcile(spacesd, cwd, image, limits)
  assert.deepEqual(workspaceListing(cwd), want)
})

const WRITE = (cwd, name, content) => ({ name: "write", arguments: { path: join(cwd, name), content } })
const BASH = (command) => ({ name: "bash", arguments: { command } })

test("transactional mode: each call commits its diff with its outcome, no checkpoints are taken, image equals workspace", async () => {
  const { dir, cwd, spacesd } = space()
  write(cwd, "preexisting.txt", "was here\n")
  const events = []
  const s = await open({
    dir,
    cwd,
    spacesd,
    epoch: 1,
    mode: "transactional",
    onRecovery: (e) => events.push(e),
    plan: [WRITE(cwd, "a.txt", "one\n"), BASH("echo two >> a.txt; echo x > b.txt; chmod 755 b.txt"), BASH("rm b.txt; mkdir -p sub; echo y > sub/c.txt")],
  })
  try {
    assert.equal((await s.run()).status, "done")
    const image = await s.harness.snapshot(ImageDoc, s.root.id, context)
    assert.deepEqual(Object.keys(image.files).sort(), ["a.txt", "preexisting.txt", "sub/c.txt"])
    assert.equal(readFileSync(join(cwd, "a.txt"), "utf8"), "one\ntwo\n")
    // The image is the workspace.
    const manifest = await scan(spacesd, cwd, limits)
    assert.deepEqual(diff(image.files, manifest), { upsert: [], remove: [] })
    assert.deepEqual(s.snapshots.calls, [], "no per-call checkpoint")
    const state = await s.harness.snapshot(SpaceDoc, s.root.id, context)
    assert.equal(state.mode, undefined)
    assert.equal(state.epoch, 1)
    assert.equal(state.checkpoints, undefined)
    assert.deepEqual(events, [])
  } finally {
    await s.close()
  }
})

test("transactional mode: the log carries only the changed bytes of each call", async () => {
  const { dir, cwd, spacesd } = space()
  write(cwd, "big.bin", "B".repeat(200_000))
  const s = await open({
    dir,
    cwd,
    spacesd,
    mode: "transactional",
    plan: [BASH("echo a >> small.txt"), BASH("echo b >> small.txt"), BASH("echo c >> small.txt")],
  })
  try {
    await s.run()
    const sizes = []
    const { DatabaseSync } = await import("node:sqlite")
    const db = new DatabaseSync(join(dir, "log.db"), { readOnly: true })
    const total = db.prepare("SELECT page_count * page_size AS bytes FROM pragma_page_count, pragma_page_size").get().bytes
    db.close()
    sizes.push(total)
    // The 200 kB file is stored once (base64: ~267 kB) plus small deltas, not once per call.
    assert.ok(total < 700_000, `log is ${total} bytes`)
  } finally {
    await s.close()
  }
})

test("transactional mode with a failing command still commits what it changed", async () => {
  const { dir, cwd, spacesd } = space()
  const s = await open({ dir, cwd, spacesd, mode: "transactional", plan: [BASH("echo partial > p.txt; exit 3")] })
  try {
    await s.run()
    const image = await s.harness.snapshot(ImageDoc, s.root.id, context)
    assert.deepEqual(Object.keys(image.files), ["p.txt"])
  } finally {
    await s.close()
  }
})

test("a Space replaced by an empty one is rebuilt from the log before the next call", async () => {
  const { dir, cwd, spacesd } = space()
  const events = []
  const plan = [BASH("echo one > a.txt"), BASH("echo two > b.txt"), BASH("cat a.txt b.txt > both.txt")]
  const first = await open({ dir, cwd, spacesd, epoch: 1, mode: "transactional", plan: plan.slice(0, 2) })
  await first.run()
  await first.close()
  // The Space is lost and replaced: empty workspace, new life (no incarnation marker).
  rmSync(cwd, { recursive: true })
  mkdirSync(cwd)
  rmSync(join(dir, "incarnation"))
  const second = await open({ dir, cwd, spacesd, epoch: 2, mode: "transactional", plan, onRecovery: (e) => events.push(e) })
  try {
    // The scripted model reads the transcript: two steps are done, the third runs.
    await second.run("again")
    assert.equal(readFileSync(join(cwd, "both.txt"), "utf8"), "one\ntwo\n")
    assert.deepEqual(events.map((e) => e.kind), ["reimaged"])
    assert.equal(events[0].mode, "transactional")
  } finally {
    await second.close()
  }
})

test("changes made to the workspace between calls join the image instead of being undone", async () => {
  const { dir, cwd, spacesd } = space()
  const plan = [BASH("echo one > a.txt"), BASH("cat user.txt >> a.txt")]
  const first = await open({ dir, cwd, spacesd, epoch: 1, mode: "transactional", plan: plan.slice(0, 1) })
  await first.run()
  await first.close()
  write(cwd, "user.txt", "from the user\n")
  const second = await open({ dir, cwd, spacesd, epoch: 2, mode: "transactional", plan })
  try {
    await second.run("again")
    assert.equal(readFileSync(join(cwd, "a.txt"), "utf8"), "one\nfrom the user\n")
    const image = await second.harness.snapshot(ImageDoc, second.root.id, context)
    assert.deepEqual(Object.keys(image.files).sort(), ["a.txt", "user.txt"])
  } finally {
    await second.close()
  }
})

test("auto picks transactional for a small container workspace and writes no checkpoint", async () => {
  const { dir, cwd, spacesd } = space()
  const s = await open({ dir, cwd, spacesd, plan: [BASH("echo hi > a.txt")] })
  try {
    await s.run()
    assert.deepEqual(s.snapshots.calls, [])
    assert.ok((await s.harness.snapshot(ImageDoc, s.root.id, context)).files["a.txt"])
  } finally {
    await s.close()
  }
})

test("mode checkpoint takes a checkpoint per call and never writes an image", async () => {
  const { dir, cwd, spacesd } = space()
  const s = await open({ dir, cwd, spacesd, mode: "checkpoint", plan: [BASH("echo hi > a.txt"), BASH("echo ho >> a.txt")] })
  try {
    await s.run()
    assert.equal(s.snapshots.calls.filter(([op]) => op === "checkpoint").length, 3, "base plus one per call")
    const image = await s.harness.snapshot(ImageDoc, s.root.id, context)
    assert.equal(image, undefined)
  } finally {
    await s.close()
  }
})

test("auto keeps checkpointing a VM whose checkpoints capture the whole machine, and remembers it", async () => {
  const { dir, cwd, spacesd } = space()
  const snapshots = new CountingSnapshots()
  snapshots.wholeMachine = true
  const events = []
  const s = await open({ dir, cwd, spacesd, kind: "microvm", snapshots, onRecovery: (e) => events.push(e), plan: [BASH("echo hi > a.txt")] })
  try {
    await s.run()
    assert.ok(snapshots.calls.length >= 2)
    const state = await s.harness.snapshot(SpaceDoc, s.root.id, context)
    assert.equal(state.mode, "checkpoint")
    assert.match(state.modeReason, /microvm/)
    assert.deepEqual(events.map((e) => [e.kind, e.mode, e.to]), [["mode", "transactional", "checkpoint"]])
  } finally {
    await s.close()
  }
})

test("auto still uses transactional mode on a microVM when only workspace checkpoints are available", async () => {
  const { dir, cwd, spacesd } = space()
  const s = await open({ dir, cwd, spacesd, kind: "microvm", plan: [BASH("echo hi > a.txt")] })
  try {
    await s.run()
    assert.deepEqual(s.snapshots.calls, [])
  } finally {
    await s.close()
  }
})

test("auto falls back to checkpoint mode for good when the workspace is over the limit before a call", async () => {
  const { dir, cwd, spacesd } = space()
  for (let i = 0; i < 6; i++) write(cwd, `f${i}`, "x")
  const events = []
  const s = await open({
    dir,
    cwd,
    spacesd,
    transactional: { maxFiles: 5 },
    onRecovery: (e) => events.push(e),
    plan: [BASH("echo hi > a.txt"), BASH("echo ho >> a.txt")],
  })
  try {
    await s.run()
    const state = await s.harness.snapshot(SpaceDoc, s.root.id, context)
    assert.equal(state.mode, "checkpoint")
    assert.match(state.modeReason, /more than 5 files/)
    assert.equal(await s.harness.snapshot(ImageDoc, s.root.id, context), undefined)
    assert.ok(s.snapshots.calls.some(([op]) => op === "checkpoint"))
    assert.equal(events.filter((e) => e.kind === "mode").length, 1)
  } finally {
    await s.close()
  }
})

test("auto falls back when a call grows the workspace past the limit: outcome and switch commit together, then a checkpoint", async () => {
  const { dir, cwd, spacesd } = space()
  const s = await open({
    dir,
    cwd,
    spacesd,
    transactional: { maxFiles: 5 },
    plan: [BASH("echo a > a.txt"), BASH("for i in 1 2 3 4 5 6 7 8; do echo $i > n$i; done"), BASH("echo z > z.txt")],
  })
  try {
    await s.run()
    const state = await s.harness.snapshot(SpaceDoc, s.root.id, context)
    assert.equal(state.mode, "checkpoint")
    assert.match(state.modeReason, /more than 5 files/)
    assert.ok(state.checkpoints.length >= 2, "the call that crossed the limit and the next one are checkpointed")
    assert.ok(state.checkpoints.some((c) => c.callId === "c1r0"))
    assert.equal(readFileSync(join(cwd, "z.txt"), "utf8"), "z\n")
  } finally {
    await s.close()
  }
})

test("auto falls back when a bash call leaves a process running in the workspace", { skip: process.platform !== "linux" && "reads /proc" }, async () => {
  const { dir, cwd, spacesd } = space()
  const s = await open({ dir, cwd, spacesd, plan: [BASH("echo a > a.txt"), BASH("(setsid sleep 30 >/dev/null 2>&1 &) ; echo started"), BASH("echo b > b.txt")] })
  try {
    await s.run()
    const state = await s.harness.snapshot(SpaceDoc, s.root.id, context)
    assert.equal(state.mode, "checkpoint")
    assert.match(state.modeReason, /left 1 process/)
  } finally {
    await s.close()
    spawnKill()
  }
})

function spawnKill() {
  try {
    // Best effort: the sleeper started by the test above.
    process.kill(-1, 0)
  } catch {}
}

test("explicit transactional mode refuses a workspace over the limit instead of falling back", async () => {
  const { dir, cwd, spacesd } = space()
  for (let i = 0; i < 6; i++) write(cwd, `f${i}`, "x")
  const s = await open({ dir, cwd, spacesd, mode: "transactional", transactional: { maxFiles: 5 }, plan: [BASH("echo hi > a.txt")] })
  try {
    const settled = await s.run()
    const messages = JSON.stringify(await s.messages())
    assert.match(messages, /more than 5 files/)
    assert.equal(workspaceListing(cwd)["a.txt"], undefined, "the command never ran")
    void settled
  } finally {
    await s.close()
  }
})

test("a conversation that already has checkpoints keeps checkpoint mode under auto", async () => {
  const { dir, cwd, spacesd } = space()
  const first = await open({ dir, cwd, spacesd, mode: "checkpoint", plan: [BASH("echo a > a.txt")] })
  await first.run()
  await first.close()
  const second = await open({ dir, cwd, spacesd, plan: [BASH("echo a > a.txt"), BASH("echo b > b.txt")] })
  try {
    await second.run("again")
    assert.equal(await second.harness.snapshot(ImageDoc, second.root.id, context), undefined)
    assert.ok(second.snapshots.calls.length > 0)
  } finally {
    await second.close()
  }
})

test("the bash tool used by these tests is the real one", () => {
  assert.equal(createBashTool().name, "bash")
})
