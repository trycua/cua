// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// The scripted "model" of the crash test and the reference model of what each
// committed tool call does to the workspace.
//
// Every step is chosen so that running it twice is visible in the final
// state: appends add a line, edits insert a line after a unique marker, and
// writes carry the step number.

import { createHash } from "node:crypto"

function rng(seed) {
  let x = seed >>> 0 || 1
  return () => {
    x ^= x << 13
    x ^= x >>> 17
    x ^= x << 5
    return (x >>> 0) / 4294967296
  }
}

/** The tool calls of one run: step i is `{ name, arguments }`. */
export function steps(seed, count) {
  const random = rng(seed)
  const plan = [{ name: "write", arguments: { path: "notes.txt", content: "BASE\n" } }]
  for (let i = 1; i < count; i++) {
    const pick = random()
    if (pick < 0.4) {
      plan.push({
        name: "bash",
        arguments: { command: `echo s${i} >> log.txt; sleep 0.${1 + Math.floor(random() * 4)}; echo appended ${i}` },
      })
    } else if (pick < 0.7) {
      plan.push({ name: "edit", arguments: { path: "notes.txt", edits: [{ oldText: "BASE\n", newText: `BASE\nE${i}\n` }] } })
    } else {
      plan.push({ name: "write", arguments: { path: `f${i % 3}.txt`, content: `v${i}-${seed}\n` } })
    }
  }
  return plan
}

/**
 * Whether a tool result is the `interrupted` result of a restored call: "The sandbox was
 * restored ..." (checkpoint mode) or "The workspace was restored ..." (transactional mode).
 */
export function isInterrupted(message) {
  return (
    message?.role === "toolResult" &&
    message.isError === true &&
    message.content.some((c) => c.type === "text" && /was restored/.test(c.text))
  )
}

/** The next assistant turn given the transcript so far: the next step, a retry, or the end. */
export function nextTurn(plan, messages) {
  const results = messages.filter((m) => m.role === "toolResult")
  const done = results.filter((m) => !isInterrupted(m)).length
  if (done >= plan.length) return { done: true }
  const retries = results.filter((m) => isInterrupted(m)).length
  const step = plan[done]
  return { done: false, call: { id: `c${done}r${retries}`, name: step.name, arguments: step.arguments } }
}

/**
 * The workspace a committed transcript implies: apply every completed call
 * (not the interrupted ones) to an in-memory file map.
 */
export function implied(messages) {
  const calls = new Map()
  for (const m of messages) {
    if (m.role !== "assistant") continue
    for (const c of m.content) if (c.type === "toolCall") calls.set(c.id, c)
  }
  const files = new Map()
  const applied = []
  // Tool results that are errors but not the interrupted wording: the call failed, so it is
  // neither applied nor retried (the plan moves on). The run must fail on them.
  const failed = []
  for (const m of messages) {
    if (m.role !== "toolResult" || isInterrupted(m)) continue
    const call = calls.get(m.toolCallId)
    if (!call) continue
    if (m.isError === true) {
      failed.push({ id: call.id, text: m.content.map((c) => c.text ?? "").join("").slice(0, 300) })
      continue
    }
    applied.push(call.id)
    const a = call.arguments
    if (call.name === "write") files.set(a.path, a.content)
    else if (call.name === "edit") {
      const text = files.get(a.path) ?? ""
      let next = text
      for (const e of a.edits) {
        if (next.split(e.oldText).length !== 2) throw new Error(`reference: ${e.oldText} not unique in ${a.path}`)
        next = next.replace(e.oldText, e.newText)
      }
      files.set(a.path, next)
    } else if (call.name === "bash") {
      const match = /^echo (s\d+) >> log\.txt;/.exec(a.command)
      if (!match) throw new Error(`reference: unknown command ${a.command}`)
      files.set("log.txt", `${files.get("log.txt") ?? ""}${match[1]}\n`)
    }
  }
  return { files, applied, failed }
}

/** `sha256sum`-style listing of a file map, sorted like the Space's `find | sort`. */
export function listing(files) {
  return [...files.keys()]
    .sort()
    .map((path) => `${createHash("sha256").update(files.get(path)).digest("hex")}  ./${path}`)
    .join("\n")
}
