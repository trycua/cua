// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// A scripted Pi Durable session on a LocalSpacesd for tests of the durable
// layer: a faux model that works through a plan of `bash` and `write` calls
// (write goes through exec, so no file daemon is needed), the log in SQLite,
// and `durableSpaces` over a local Space.

import { BACKGROUND_CONTEXT as context } from "@earendil-works/chord/context"
import { createModels, fauxAssistantMessage, fauxProvider, fauxText, fauxToolCall } from "@earendil-works/pi-ai"
import { createRegistry, defineExtension, defineTool, Harness } from "@earendil-works/pi-durable"
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node"
import { createBashTool } from "@earendil-works/pi-durable/tools"
import { Type } from "typebox"
import { durableSpaces, openFencedSqlite } from "../dist/index.js"
import { CountingSnapshots, localSpaces } from "./local-space.mjs"

export { context }

/** `write` through the shell: `printf %s "$C" > path`, so it needs only exec. */
const writeTool = defineTool({
  name: "write",
  description: "Write a file",
  parameters: Type.Object({ path: Type.String(), content: Type.String() }),
  async execute(args, api, ctx) {
    const result = await api.env.exec(
      ["sh", "-c", 'mkdir -p "$(dirname "$P")" && printf %s "$C" > "$P"'],
      { env: { P: args.path, C: args.content } },
      ctx,
    )
    if (!result.ok) throw result.error
    return { content: [{ type: "text", text: `wrote ${args.path}` }] }
  },
})

export const tools = defineExtension({ name: "test-tools", tools: [createBashTool(), writeTool] })

export function isInterrupted(message) {
  return (
    message?.role === "toolResult" &&
    message.isError === true &&
    message.content.some((c) => c.type === "text" && /was restored/.test(c.text))
  )
}

/** The next assistant turn: the next unfinished step of `plan`, or the end. */
export function nextTurn(plan, messages) {
  const results = messages.filter((m) => m.role === "toolResult")
  const done = results.filter((m) => !isInterrupted(m)).length
  if (done >= plan.length) return { done: true }
  const retries = results.filter((m) => isInterrupted(m)).length
  const step = plan[done]
  return { done: false, call: { id: `c${done}r${retries}`, name: step.name, arguments: step.arguments } }
}

/**
 * Opens a session over `dir/log.db` and the Space at `spacesd`.
 * `epoch` fences the log and the Space; omit it for an unfenced session.
 */
export async function open({
  dir,
  cwd,
  spacesd,
  plan,
  epoch,
  mode,
  snapshots = new CountingSnapshots(),
  trace,
  onRecovery,
  kind,
  transactional,
  models: extraModels,
  onHalt,
  onStale,
  spaces: spacesOverride,
  storageOverride,
  durableOverride,
}) {
  let fence
  let storage
  if (storageOverride) {
    storage = storageOverride
  } else if (epoch !== undefined) {
    const opened = await openFencedSqlite(`${dir}/log.db`, epoch)
    storage = opened.storage
    fence = opened.fence
  } else {
    storage = await openNodeSqliteStorage(`${dir}/log.db`)
  }
  const durable = durableOverride ?? durableSpaces({
    spaces: spacesOverride ?? localSpaces(spacesd, kind ? { kind } : {}),
    snapshots,
    cwd,
    spaceName: () => "test-space",
    incarnationMarker: `${dir}/incarnation`,
    ...(epoch !== undefined ? { epoch, fence } : {}),
    ...(mode ? { mode } : {}),
    ...(transactional ? { transactional } : {}),
    ...(trace ? { trace } : {}),
    ...(onHalt ? { onHalt } : {}),
    ...(onStale ? { onStale } : {}),
    ...(onRecovery ? { onRecovery } : {}),
  })
  const faux = fauxProvider()
  const models = extraModels ?? createModels()
  models.setProvider(faux.provider)
  faux.setResponses(
    Array.from({ length: 500 }, () => (transcript) => {
      const turn = nextTurn(plan, transcript.messages)
      if (turn.done) return fauxAssistantMessage([fauxText("done")])
      return fauxAssistantMessage([fauxToolCall(turn.call.name, turn.call.arguments, { id: turn.call.id })], {
        stopReason: "toolUse",
      })
    }),
  )
  const registry = createRegistry()
  registry.install(tools)
  registry.install(durable.extension)
  const harness = await Harness.open(storage, { models, registry, env: durable.env }, context)
  harness.resume()
  const root = await harness.root(context, { agent: { model: { provider: "faux", modelId: "faux-1" }, cwd } })
  return {
    harness,
    root,
    durable,
    snapshots,
    fence,
    storage,
    async run(requestId = "run") {
      const submission = await root.submit({ type: "input", content: "run the plan", requestId }, context)
      return submission.wait(context)
    },
    async messages() {
      return (await root.context(context)).messages
    },
    async close() {
      await harness.close(context).catch(() => {})
      await durable.close()
    },
  }
}
