// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest"
import { approvalOf, awaitingApproval, lastLine, statusLabel, toBlocks, type TranscriptLine } from "./blocks"

// What the core sends: user lines it kept, and the SDK's fold of the run's
// events (agent messages and activity groups).
const lines: TranscriptLine[] = [
  { turn: 0, speaker: "activity", text: "2 steps", steps: ["Install node: cached", "Install claude-code: done"] },
  { turn: 1, speaker: "user", text: "tidy the downloads" },
  { turn: 1, speaker: "agent", text: "Looking at ~/Downloads." },
  { turn: 1, speaker: "activity", text: "2 steps", steps: ["Tool Terminal completed: 14 files", "Thinking: sort them"] },
  { turn: 1, speaker: "agent", text: "Delete the 3 duplicates? (y/n)" },
]

describe("thread blocks", () => {
  it("messages are text, activity is one muted group per run of steps", () => {
    const b = toBlocks(lines, [{ turn: 1, name: "a.pdf", detail: "1 KB" }])
    expect(b.map((x) => x.kind)).toEqual(["bot", "user", "bot"])
    expect(b[0].kind === "bot" && b[0].items).toEqual([
      { kind: "activity", summary: "2 steps", steps: ["Install node: cached", "Install claude-code: done"] },
    ])
    expect(b[1]).toEqual({ kind: "user", turn: 1, text: "tidy the downloads" })
    const items = b[2].kind === "bot" ? b[2].items : []
    expect(items.map((i) => i.kind)).toEqual(["text", "activity", "approval", "file"])
    expect(items[0]).toEqual({ kind: "text", text: "Looking at ~/Downloads." })
  })

  it("an approval is pending only while it is last", () => {
    expect(awaitingApproval(toBlocks(lines))).toBe(true)
    expect(awaitingApproval(toBlocks([...lines, { turn: 2, speaker: "user", text: "y" }]))).toBe(false)
  })

  it("recognises approvals", () => {
    expect(approvalOf("Proceed? [Y/n]")).toEqual({ prompt: "Proceed?", options: ["Y", "n"] })
    expect(approvalOf("no question here")).toBeNull()
  })

  it("the sidebar shows the SDK preview, or the user's own last words", () => {
    expect(lastLine(lines, "Delete the 3 duplicates? (y/n)")).toBe("Delete the 3 duplicates? (y/n)")
    expect(lastLine([{ turn: 1, speaker: "user", text: "hi" }], null)).toBe("You: hi")
    // Activity after the prompt (installs, a turn end) is not the Bot speaking.
    expect(
      lastLine(
        [
          { turn: 1, speaker: "user", text: "Watch the dashboards." },
          { turn: 1, speaker: "agent", text: "Watching." },
          { turn: 1, speaker: "activity", text: "1 step", steps: ["Turn 1 ended (end_turn)"] },
        ],
        "Watching.",
      ),
    ).toBe("Watching.")
    expect(lastLine([], undefined)).toBe("")
    expect(statusLabel("running")).toBe("Working")
    expect(statusLabel(null)).toBe("Ready")
  })
})
