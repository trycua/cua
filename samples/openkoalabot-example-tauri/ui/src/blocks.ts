// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Bot's transcript as the thread renders it: user turns as bubbles, the
// Bot's messages as plain text (plus approval and file cards), and its
// activity (install, tools, thinking, turn ends, notices) as muted,
// collapsible step groups. Which is which comes from the SDK's event
// classification (the core's transcript), never from matching text here.
// DOM-free so it is unit-tested.

import type { Line } from "./logic"

export type TranscriptLine = Line

/** A file the operator sent into the Space during `turn`. */
export interface FileEvent {
  turn: number
  name: string
  detail: string
}

export type Item =
  | { kind: "text"; text: string }
  | { kind: "activity"; summary: string; steps: string[] }
  | { kind: "approval"; prompt: string; options: string[] }
  | { kind: "file"; name: string; detail: string }

export type Block = { kind: "user"; turn: number; text: string } | { kind: "bot"; turn: number; items: Item[] }

/** A trailing yes/no question (`Proceed? (y/n)`), the Bot asking to go on. */
export function approvalOf(text: string): { prompt: string; options: string[] } | null {
  const m = text.trim().match(/^(.*?)\s*[[(]\s*(y(?:es)?)\s*\/\s*(n(?:o)?)\s*[\])]\s*[?:]?\s*$/i)
  if (!m) return null
  return { prompt: m[1].trim() || text.trim(), options: [m[2], m[3]] }
}

function pushText(items: Item[], text: string): void {
  const last = items[items.length - 1]
  if (last && last.kind === "text") last.text += `\n${text}`
  else items.push({ kind: "text", text })
}

/** Groups transcript lines (and file events) into thread blocks. An
 *  approval card only stays actionable when it is the thread's last item. */
export function toBlocks(lines: TranscriptLine[], files: FileEvent[] = []): Block[] {
  const out: Block[] = []
  const bot = (turn: number): Item[] => {
    const last = out[out.length - 1]
    if (last && last.kind === "bot" && last.turn === turn) return last.items
    const b: Block = { kind: "bot", turn, items: [] }
    out.push(b)
    return b.items
  }
  const pendingFiles = [...files].sort((a, b) => a.turn - b.turn)
  const flushFiles = (upTo: number) => {
    while (pendingFiles.length && pendingFiles[0].turn <= upTo) {
      const f = pendingFiles.shift()!
      bot(f.turn).push({ kind: "file", name: f.name, detail: f.detail })
    }
  }
  for (const l of lines) {
    if (l.speaker === "user") {
      flushFiles(l.turn - 1)
      out.push({ kind: "user", turn: l.turn, text: l.text })
      continue
    }
    const items = bot(l.turn)
    if (l.speaker === "activity") {
      items.push({ kind: "activity", summary: l.text, steps: l.steps ?? [] })
      continue
    }
    const approval = approvalOf(l.text)
    if (approval) items.push({ kind: "approval", ...approval })
    else pushText(items, l.text)
  }
  flushFiles(Number.MAX_SAFE_INTEGER)
  return out
}

/** Whether the thread ends on an approval request (the roster badge). */
export function awaitingApproval(blocks: Block[]): boolean {
  const last = blocks[blocks.length - 1]
  if (!last || last.kind !== "bot") return false
  const item = last.items[last.items.length - 1]
  return item?.kind === "approval"
}

/** The roster row's last line: "You: ..." while the user spoke last, else the
 *  SDK's preview (the agent's last message; activity is never a preview). */
export function lastLine(lines: TranscriptLine[], preview?: string | null): string {
  for (let i = lines.length - 1; i >= 0; i--) {
    const l = lines[i]
    if (l.speaker === "activity" || !l.text.trim()) continue
    if (l.speaker === "user") return `You: ${l.text.trim()}`
    break
  }
  return preview?.trim() ?? ""
}

/** Human status for a run state. */
export function statusLabel(state: string | null | undefined): string {
  switch (state) {
    case null:
    case undefined:
    case "":
      return "Ready"
    case "running":
      return "Working"
    case "idle":
    case "finished":
      return "Done"
    case "awaiting_input":
      return "Needs you"
    case "failed":
    case "crashed":
      return "Failed"
    case "stopped":
      return "Stopped"
    default:
      return state.replace(/_/g, " ")
  }
}
