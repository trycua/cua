// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// Calls the wrapper `durableSpaces` puts around a tool directly, with a
// minimal stand-in for Pi's tool execution API that keeps documents in memory.
// This is for unit tests of one call's behavior; the Harness-driven tests
// (fixture.mjs) cover the integration.

import { BACKGROUND_CONTEXT as context } from "@earendil-works/chord/context"

export function fakeApi({ conversationId = 7, taskId = 100, callId = "call-1" } = {}) {
  const docs = new Map()
  const key = (token, id) => `${token.definition.kind}:${id}`
  const outputs = []
  const commits = []
  const api = {
    conversationId,
    taskId,
    callId,
    outputs,
    commits,
    docs,
    output(chunk) {
      outputs.push(typeof chunk === "string" ? chunk : new TextDecoder().decode(chunk))
    },
    async details() {},
    async diagnostic() {},
    async snapshot(token, id) {
      const doc = docs.get(key(token, id))
      return doc === undefined ? undefined : structuredClone(doc)
    },
    async getTask() {
      return undefined
    },
    async commit(change) {
      const draft = structuredClone([...docs])
      const staged = new Map(draft)
      const tx = {
        async doc(token, id) {
          const k = key(token, id)
          if (!staged.has(k)) staged.set(k, token.definition.initial())
          return staged.get(k)
        },
      }
      await change(tx)
      for (const [k, v] of staged) docs.set(k, v)
      commits.push([...staged.keys()])
    },
  }
  return api
}

/** The wrapped version of `tool` under `durable.extension`. */
export function wrapped(durable, tool) {
  const wrap = durable.extension.wraps.find((w) => w.tool === tool.name)
  return wrap.wrap(tool)
}

export { context }
