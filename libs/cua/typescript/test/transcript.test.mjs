// The Rust event classification and transcript fold, through the native
// binding (hermetic: no Space, JSON pages only).
import assert from "node:assert/strict"
import { test } from "node:test"

import { AgentTranscript, agentEventCategory } from "../dist/index.js"

test("event kinds classify into message, user, activity and hidden", () => {
  assert.equal(agentEventCategory("message"), "message")
  assert.equal(agentEventCategory("turn_started"), "user")
  assert.equal(agentEventCategory("install"), "activity")
  assert.equal(agentEventCategory("tool_update"), "activity")
  assert.equal(agentEventCategory("turn_ended"), "activity")
  assert.equal(agentEventCategory("usage"), "hidden")
})

test("an agent_events page folds into bubbles and one muted group", () => {
  const t = new AgentTranscript()
  const page = {
    run_id: "r",
    status: "idle",
    cursor: 900,
    caught_up: true,
    events: [
      { seq: 1, ts_ms: 0, turn: 0, kind: "install", text: "node cached " },
      { seq: 2, ts_ms: 0, turn: 1, kind: "turn_started", text: "hi" },
      { seq: 3, ts_ms: 0, turn: 1, kind: "user_message", text: "hi" },
      { seq: 4, ts_ms: 0, turn: 1, kind: "message", text: "Hello " },
      { seq: 5, ts_ms: 0, turn: 1, kind: "message", text: "there." },
      { seq: 6, ts_ms: 0, turn: 1, kind: "tool_call", tool_id: "t", tool_title: "mcp__cua-driver__click", tool_status: "pending" },
      { seq: 7, ts_ms: 0, turn: 1, kind: "tool_update", tool_id: "t", tool_status: "completed", text: "{}" },
      { seq: 8, ts_ms: 0, turn: 1, kind: "turn_ended", stop_reason: "end_turn" },
    ],
  }
  assert.equal(t.absorbJson(JSON.stringify(page)), 900n)
  const rev = t.revision()
  t.absorbJson(JSON.stringify(page)) // a re-read page changes nothing
  assert.equal(t.revision(), rev)
  const items = t.items()
  assert.deepEqual(
    items.map((i) => [i.kind, i.turn, i.text]),
    [
      ["activity", 0, "1 step"],
      ["user", 1, "hi"],
      ["message", 1, "Hello there."],
      ["activity", 1, "2 steps"],
    ],
  )
  assert.deepEqual(items[3].steps, ["Tool mcp__cua-driver__click completed: {}", "Turn 1 ended (end_turn)"])
  assert.equal(t.preview(), "Hello there.")
  assert.equal(t.cursor(), 900n)
  t.note(1, "queued while working")
  assert.equal(t.items()[3].text, "3 steps")
  assert.throws(() => t.absorbJson("42"))
})
