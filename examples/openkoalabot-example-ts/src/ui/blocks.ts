// A Bot's thread as the page renders it: user turns as bubbles, the Bot's
// messages as prose (a trailing yes/no question as an approval card), its
// activity (install, tools, thinking, turn ends, refusals) as one muted,
// collapsible group per run of steps, and file cards. The server sends the
// items the SDK's event fold made (`AgentTranscript`); nothing here parses
// harness text. DOM-free so it is unit-tested.

/** One thread item from the server (`BotThread.items`). */
export interface ThreadItem {
  turn: number
  kind: "user" | "message" | "activity"
  /** The message or prompt; for activity, the group summary (`5 steps`). */
  text: string
  /** Activity only: one line per step. */
  steps: string[]
}

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

/** Groups thread items (and file events) into blocks. An approval card only
 *  stays actionable when it is the thread's last item. */
export function toBlocks(items: ThreadItem[], files: FileEvent[] = []): Block[] {
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
  for (const it of items) {
    if (it.kind === "user") {
      flushFiles(it.turn - 1)
      out.push({ kind: "user", turn: it.turn, text: it.text })
    } else if (it.kind === "activity") {
      bot(it.turn).push({ kind: "activity", summary: it.text, steps: [...it.steps] })
    } else {
      const text = it.text.trim()
      if (!text) continue
      // A question on the message's last line becomes the approval card.
      const lines = text.split("\n")
      const approval = approvalOf(lines[lines.length - 1])
      const target = bot(it.turn)
      if (approval) {
        const before = lines.slice(0, -1).join("\n").trim()
        if (before) target.push({ kind: "text", text: before })
        target.push({ kind: "approval", ...approval })
      } else target.push({ kind: "text", text })
    }
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

/** The roster row's second line: the Bot's last message (the SDK's
 *  preview, never install progress or a turn end), else the user's last
 *  prompt. */
export function rowPreview(preview: string, items: ThreadItem[]): string {
  if (preview.trim()) return preview.trim()
  const lastUser = [...items].reverse().find((i) => i.kind === "user" && i.text.trim())
  return lastUser ? `You: ${lastUser.text.trim()}` : ""
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
