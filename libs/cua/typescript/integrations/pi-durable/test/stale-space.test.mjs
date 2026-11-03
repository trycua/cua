// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// Epoch fencing and the workspace image against a real cua-spacesd (opt-in:
// see space.mjs): a spawn with a stale epoch is refused and surfaces as
// StaleEpochError, advancing the epoch kills older commands, and the image of
// a real workspace can be scanned, diffed and reconciled.

import assert from "node:assert/strict"
import { randomUUID } from "node:crypto"
import { after, before, test } from "node:test"
import { BACKGROUND_CONTEXT as context } from "@earendil-works/chord/context"
import { SpacesExecutionEnv, StaleEpochError, advanceEpoch, getExecRecord } from "../dist/index.js"
import { sh } from "../dist/spacesd.js"
import { applyDiff, capture, DEFAULT_TRANSACTIONAL_LIMITS as limits, diff, reconcile, scan } from "../dist/transactional.js"
import { connectSpace, SPACE_URL } from "./space.mjs"

const skip = SPACE_URL ? false : "set CUA_PI_SPACESD_URL"
let spacesd
let cwd
let epoch = 0

before(async () => {
  if (skip) return
  spacesd = await connectSpace()
  cwd = `/tmp/pi-stale-${randomUUID()}`
  assert.equal((await sh(spacesd, `mkdir -p ${cwd}`)).code, 0)
  epoch = (await getExecRecord(spacesd, "probe")).epoch + 1
  await advanceEpoch(spacesd, epoch)
})

after(async () => {
  if (spacesd) await sh(spacesd, `rm -rf ${cwd}`)
})

const env = () => new SpacesExecutionEnv({ spacesd, id: `cua-space:stale:${cwd}`, cwd, epoch: () => epoch })

test("a spawn with a stale epoch is refused by the Space and surfaces as StaleEpochError, not a result", { skip }, async () => {
  const seen = []
  const view = new SpacesExecutionEnv({ spacesd, id: "cua-space:stale", cwd, epoch: () => epoch, onStale: (e) => seen.push(e) }).withRequestIds(`stale-${randomUUID()}`)
  assert.equal((await view.exec("true", {}, context)).ok, true)
  await advanceEpoch(spacesd, epoch + 1)
  epoch += 1
  const old = new SpacesExecutionEnv({ spacesd, id: "cua-space:stale", cwd, epoch: () => epoch - 1, onStale: (e) => seen.push(e) })
  await assert.rejects(old.exec("echo late > late.txt", {}, context), (error) => error instanceof StaleEpochError && error.epoch === epoch - 1 && error.floor === epoch)
  assert.equal(seen.length, 1)
  assert.equal((await sh(spacesd, `test -e ${cwd}/late.txt`)).code, 1, "the stale command never ran")
})

test("advancing the epoch kills a running older command, whose ledger record is then not adoptable", { skip }, async () => {
  const prefix = `kill-${randomUUID()}`
  const view = env().withRequestIds(prefix)
  const running = view.exec("sleep 30", {}, context)
  await new Promise((resolve) => setTimeout(resolve, 500))
  await advanceEpoch(spacesd, epoch + 1)
  epoch += 1
  await running
  const { record } = await getExecRecord(spacesd, `${prefix}:0@${epoch - 1}`)
  assert.equal(record.state, "exited")
  assert.ok(record.signal !== undefined || record.exitCode === undefined, "killed, not a clean exit")
})

test("the workspace image of a real Space: scan, diff, and reconcile after damage", { skip }, async () => {
  const root = `${cwd}/img`
  const setup = [
    `mkdir -p ${root}/sub`,
    `printf one > ${root}/a.txt`,
    `printf two > '${root}/sub/it'"'"'s b.txt'`,
    `printf '#!/bin/sh\\n' > ${root}/run.sh && chmod 755 ${root}/run.sh`,
    `ln -s a.txt ${root}/ln`,
  ].join(" && ")
  assert.equal((await sh(spacesd, setup)).code, 0)
  const manifest = await scan(spacesd, root, limits)
  assert.deepEqual(Object.keys(manifest).sort(), ["a.txt", "ln", "run.sh", "sub/it's b.txt"])
  const image = {}
  applyDiff(image, manifest, diff(undefined, manifest), await capture(spacesd, root, manifest, Object.keys(manifest), new Set()))
  assert.equal(
    (await sh(spacesd, `rm ${root}/a.txt; printf junk > ${root}/extra; printf x > '${root}/sub/it'"'"'s b.txt'; chmod 644 ${root}/run.sh`)).code,
    0,
  )
  const done = await reconcile(spacesd, root, image, limits)
  assert.ok(done.written >= 3 && done.deleted === 1, JSON.stringify(done))
  assert.deepEqual(diff(image.files, await scan(spacesd, root, limits)), { upsert: [], remove: [] })
})
