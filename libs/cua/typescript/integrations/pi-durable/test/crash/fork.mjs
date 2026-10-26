// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// Forks: a conversation forked at a tool-result entry gets its own Space,
// forked from the checkpoint its parent recorded at that entry; the parent's
// Space is untouched by the fork and the fork by the parent's later calls.
//
// Usage: node test/crash/fork.mjs   (Docker and the cua-pi-space image, like run.mjs)

import assert from "node:assert/strict"
import { randomBytes } from "node:crypto"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { BACKGROUND_CONTEXT as context } from "@earendil-works/chord/context"
import { createModels, fauxAssistantMessage, fauxProvider, fauxText, fauxToolCall } from "@earendil-works/pi-ai"
import { createRegistry, Harness, ToolResultEntry } from "@earendil-works/pi-durable"
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node"
import { CodingTools } from "@earendil-works/pi-durable/tools"
import { durableSpaces, SpaceDoc, WorkspaceSnapshots } from "../../dist/index.js"
import { sh } from "../../dist/spacesd.js"
import { DockerSpaces } from "./docker-spaces.mjs"

const WORK = "/work"
const dir = mkdtempSync(join(tmpdir(), "pi-fork-"))
process.env.CUA_HOME = join(dir, "cua-home")
const spaces = new DockerSpaces({ token: randomBytes(16).toString("hex") })
const prefix = `fork-${Date.now()}`
const durable = durableSpaces({
  spaces,
  snapshots: new WorkspaceSnapshots({ spaces, root: WORK }),
  cwd: WORK,
  spaceName: (id) => `${prefix}-${id}`,
  onRecovery: (event) => console.log("recovery", JSON.stringify(event)),
})

const parentPlan = [
  { name: "write", arguments: { path: "a.txt", content: "parent-1\n" } },
  { name: "bash", arguments: { command: "echo s1 >> log.txt" } },
  { name: "write", arguments: { path: "b.txt", content: "parent-after-fork\n" } },
]
const forkPlan = [{ name: "write", arguments: { path: "c.txt", content: "fork-only\n" } }]

const faux = fauxProvider()
const models = createModels()
models.setProvider(faux.provider)
const respond = (transcript) => {
  const messages = transcript.messages
  const inFork = messages.some((m) => m.role === "user" && JSON.stringify(m.content).includes("in the fork"))
  const lastUser = messages.findLastIndex((m) => m.role === "user")
  const plan = inFork ? forkPlan : parentPlan
  const done = messages.slice(inFork ? lastUser : 0).filter((m) => m.role === "toolResult").length
  if (done >= plan.length) return fauxAssistantMessage([fauxText("done")])
  const step = plan[done]
  const id = `${inFork ? "f" : "p"}${done}`
  return fauxAssistantMessage([fauxToolCall(step.name, step.arguments, { id })], { stopReason: "toolUse" })
}
faux.setResponses(Array.from({ length: 100 }, () => respond))

const registry = createRegistry()
registry.install(CodingTools)
registry.install(durable.extension)
const harness = await Harness.open(
  await openNodeSqliteStorage(join(dir, "session.sqlite")),
  { models, registry, env: durable.env },
  context,
)
harness.resume()

async function listing(name) {
  const result = await sh(await spaces.spacesd(name), `cd ${WORK} && for f in $(ls | LC_ALL=C sort); do echo "$f=$(cat $f | tr '\\n' ' ')"; done`)
  return result.stdout.trim()
}

try {
  const root = await harness.root(context, { agent: { model: { provider: "faux", modelId: "faux-1" }, cwd: WORK } })
  await (await root.submit({ type: "input", content: "parent" }, context)).wait(context)
  const page = await root.entries({}, 100, undefined, context)
  const entries = [...page.items].reverse()
  const results = entries.filter((e) => ToolResultEntry.is(e))
  const afterBash = results.find((e) => e.model[0].toolCallId === "p1")
  assert.ok(afterBash, "the bash call's result entry")

  const parentSpace = durable.spaceOf(root.id)
  const parentState = await harness.snapshot(SpaceDoc, root.id, context)
  console.log("parent checkpoints", JSON.stringify(parentState.checkpoints))

  const fork = await root.fork(afterBash.id, { ownership: { kind: "ownerless" } }, context)
  await (await fork.submit({ type: "input", content: "continue in the fork" }, context)).wait(context)
  const forkSpace = durable.spaceOf(fork.id)
  const forkState = await harness.snapshot(SpaceDoc, fork.id, context)
  console.log("fork checkpoints", JSON.stringify(forkState.checkpoints))

  const parentFiles = await listing(parentSpace)
  const forkFiles = await listing(forkSpace)
  console.log(`parent ${parentSpace}: ${parentFiles}`)
  console.log(`fork   ${forkSpace}: ${forkFiles}`)
  assert.notEqual(forkSpace, parentSpace)
  assert.equal(parentFiles, "a.txt=parent-1 \nb.txt=parent-after-fork \nlog.txt=s1")
  assert.equal(forkFiles, "a.txt=parent-1 \nc.txt=fork-only \nlog.txt=s1")
  assert.equal(forkState.owner, fork.id)
  console.log("FORK OK")
} finally {
  await harness.close(context)
  await durable.close()
  for (const name of [durable.spaceOf(1), durable.spaceOf(2)]) if (name) await spaces.remove(name)
  rmSync(dir, { recursive: true, force: true })
}
