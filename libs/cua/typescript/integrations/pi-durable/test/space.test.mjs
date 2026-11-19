// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// The adapter against a real cua-spacesd (opt-in: see space.mjs): idempotent
// commands, the exec ledger, fencing, and workspace checkpoints.

import assert from "node:assert/strict"
import { randomUUID } from "node:crypto"
import { after, before, test } from "node:test"
import { BACKGROUND_CONTEXT as context } from "@earendil-works/chord/context"
import { SpacesExecutionEnv, StaleEpochError, WorkspaceSnapshots, advanceEpoch, getExecRecord } from "../dist/index.js"
import { sh } from "../dist/spacesd.js"
import { connectSpace, SPACE_URL } from "./space.mjs"

const skip = SPACE_URL ? false : "set CUA_PI_SPACESD_URL"
let spacesd
let cwd
let epoch = 0

before(async () => {
  if (skip) return
  spacesd = await connectSpace()
  cwd = `/tmp/pi-space-${randomUUID()}`
  assert.equal((await sh(spacesd, `mkdir -p ${cwd}`)).code, 0)
  // Epoch 0 means unfenced: start fenced, as durableSpaces does.
  epoch = (await getExecRecord(spacesd, "probe")).epoch + 1
  await advanceEpoch(spacesd, epoch)
})

after(async () => {
  if (spacesd) await sh(spacesd, `rm -rf ${cwd}`)
})

function env(extra = {}) {
  return new SpacesExecutionEnv({ spacesd, id: `cua-space:test:${cwd}`, cwd, epoch: () => epoch, ...extra })
}

async function run(view, command) {
  let output = ""
  const result = await view.exec(command, { onOutput: (text) => (output += text) }, context)
  return { result, output }
}

test("a command with a request id runs once; a retry replays its outcome", { skip }, async () => {
  const prefix = `idem-${randomUUID()}`
  const command = "echo run >> runs.txt; printf 'out\\n'; exit 3"
  const first = await run(env().withRequestIds(prefix), command)
  const again = await run(env().withRequestIds(prefix), command)
  assert.deepEqual(first.result, { ok: true, value: { exitCode: 3 } })
  assert.deepEqual(again.result, first.result)
  assert.equal(again.output, "out\n")
  const runs = await sh(spacesd, `wc -l < ${cwd}/runs.txt`)
  assert.equal(runs.stdout.trim(), "1")
  const { record } = await getExecRecord(spacesd, `${prefix}:0@${epoch}`)
  assert.equal(record.state, "exited")
  assert.equal(record.exitCode, 3)
  assert.equal(record.outputBytes, 4)
})

test("a retry while the command still runs reattaches instead of starting it again", { skip }, async () => {
  const prefix = `attach-${randomUUID()}`
  const command = "echo run >> attach.txt; sleep 1; echo finished"
  const [a, b] = await Promise.all([
    run(env().withRequestIds(prefix), command),
    new Promise((resolve) => setTimeout(resolve, 300)).then(() => run(env().withRequestIds(prefix), command)),
  ])
  assert.deepEqual(a.result, { ok: true, value: { exitCode: 0 } })
  assert.deepEqual(b.result, a.result)
  assert.match(b.output, /finished/)
  assert.equal((await sh(spacesd, `wc -l < ${cwd}/attach.txt`)).stdout.trim(), "1")
})

test("advancing the epoch kills fenced commands and refuses the old epoch", { skip }, async () => {
  const view = env().withRequestIds(`fence-${randomUUID()}`)
  const running = run(view, "sleep 30")
  await new Promise((resolve) => setTimeout(resolve, 500))
  const advanced = await advanceEpoch(spacesd, epoch + 1)
  assert.equal(advanced.epoch, epoch + 1)
  assert.ok(advanced.killed >= 1)
  const killed = await running
  assert.deepEqual(killed.result, { ok: true, value: { exitCode: 137 } })
  await assert.rejects(run(view, "true"), (error) => error instanceof StaleEpochError)
  epoch += 1
  assert.deepEqual((await run(env(), "true")).result, { ok: true, value: { exitCode: 0 } })
})

test("files go through pi-env and a workspace checkpoint restores them exactly", { skip }, async () => {
  const fs = env()
  const root = `${cwd}/ws`
  assert.equal((await sh(spacesd, `mkdir -p ${root}`)).code, 0)
  fs.cwd = root
  assert.equal((await fs.writeFile("a.txt", "one\n", context)).ok, true)
  assert.equal((await fs.createDir("sub/deep", { recursive: true }, context)).ok, true)
  assert.equal((await fs.writeFile("sub/deep/b.txt", "two\n", context)).ok, true)
  const spaces = { exists: async () => true, ensure: async () => {}, spacesd: async () => spacesd }
  const snapshots = new WorkspaceSnapshots({ spaces, root, guestDir: `${cwd}/store` })
  const id = await snapshots.checkpoint("s", `t-${randomUUID()}`)
  assert.equal(await snapshots.checkpoint("s", id), id, "idempotent by label")
  assert.equal((await fs.writeFile("a.txt", "changed\n", context)).ok, true)
  assert.equal((await fs.remove("sub", { recursive: true }, context)).ok, true)
  assert.equal((await fs.writeFile("new.txt", "x", context)).ok, true)
  await fs.close()
  await snapshots.restore("s", id)
  const after = env()
  after.cwd = root
  assert.equal((await after.readTextFile("a.txt", context)).value, "one\n")
  assert.equal((await after.readTextFile("sub/deep/b.txt", context)).value, "two\n")
  assert.equal((await after.exists("new.txt", context)).value, false)
  await after.close()
})
