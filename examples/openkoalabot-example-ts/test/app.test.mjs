import assert from "node:assert/strict"
import { test } from "node:test"
import { BotStore, BotThread } from "../dist/core/app.js"
import { waitForPresence } from "@trycua/cua/spaces/presence"
import { FakeSpace } from "./fake.mjs"

const koala = { id: "koala", name: "Koala", agent: "claude-code" }

test("a thread: hire, settle, follow up, and output attributed per turn", async () => {
  const space = new FakeSpace()
  const store = new BotStore(space)
  const t = await store.hire(koala, "open koala")
  assert.equal(t.state, "starting")
  assert.deepEqual(t.items.map((i) => i.kind), ["user", "activity"], "the prompt, then the start note as activity")
  assert.equal(await t.settle(1, 20), "idle")
  assert.deepEqual(t.outputOf(0), ["did: open koala"])
  const d = await t.send("again")
  assert.deepEqual(d, { accepted: true, reason: "a new turn" })
  assert.equal(await t.settle(1, 20, (th) => th.outputOf(1).length > 0), "idle")
  assert.deepEqual(t.outputOf(1), ["continued: again"], "the new line belongs to turn 1, the old one stays on turn 0")
  assert.deepEqual(t.outputOf(0), ["did: open koala"])
  assert.equal(t.turns.length, 2)
  assert.deepEqual(
    t.items.map((i) => [i.turn, i.kind]),
    [[0, "user"], [0, "activity"], [0, "message"], [0, "activity"], [1, "user"], [1, "message"], [1, "activity"]],
  )
  assert.equal(t.items.filter((i) => i.kind === "user").length, 2, "the agent's copy of each prompt is not shown again")
  assert.equal(t.preview, "continued: again")
})

test("install, tool and turn-end events are activity, never a message or the preview", async () => {
  const space = new FakeSpace()
  space.startEvents = [{ kind: "install", text: "node cached ", summary: "Install node: cached" }]
  const t = await new BotStore(space).hire(koala, "check memory")
  space.runs.get(t.runId).pending = [
    { kind: "tool_call", tool_id: "c1", tool_title: "mcp__cua-driver__click", tool_status: "pending" },
    { kind: "tool_update", tool_id: "c1", tool_status: "completed", text: '{"ok":true}' },
    "4 GiB total.",
    { kind: "thought", text: "done now" },
  ]
  await t.settle(1, 20)
  const items = t.items
  // Setup (turn 0) and the turn's own work are the SDK's two groups.
  assert.deepEqual(items.map((i) => i.kind), ["user", "activity", "activity", "message", "activity"])
  assert.deepEqual(items[1].steps, ["no host credentials copied", "Install node: cached"])
  assert.equal(items[1].text, "2 steps")
  assert.deepEqual(items[2].steps, ['Tool mcp__cua-driver__click completed: {"ok":true}'])
  assert.equal(items[2].text, "1 step")
  assert.deepEqual(items[4].steps, ["Thinking: done now", "Turn 1 ended (end_turn)"])
  assert.equal(t.preview, "4 GiB total.")
  assert.deepEqual(t.outputOf(0), ["4 GiB total."])
  assert.ok(!items.some((i) => i.kind === "message" && /^\[(install|tool|turn)/.test(i.text)))
})

test("a message to a running Bot is refused and shown, never queued", async () => {
  const space = new FakeSpace()
  const t = await new BotStore(space).hire(koala, "slow")
  const d = await t.send("hurry")
  assert.equal(d.accepted, false)
  assert.match(d.reason, /refused rather than queued/)
  assert.ok(!space.calls.includes("agentMessage"), "nothing was sent")
  const last = t.items.at(-1)
  assert.equal(last.kind, "activity")
  assert.match(last.steps.at(-1), /^Refused:/)
})

test("a failed status probe degrades to unknown, never to health", async () => {
  const space = new FakeSpace()
  const t = await new BotStore(space).hire(koala, "x")
  space.failStatus = true
  assert.equal(await t.refresh(), "unknown")
  assert.equal(t.acceptsMessage, false)
  assert.match(t.reason, /probe failed/)
})

test("settle is bounded", async () => {
  const space = new FakeSpace()
  const t = await new BotStore(space).hire(koala, "x")
  space.failStatus = true
  await assert.rejects(t.settle(1, 3), /did not settle after 3 polls/)
})

test("the roster is one agentList per tick and joins runs back to Bots", async () => {
  const space = new FakeSpace()
  const store = new BotStore(space)
  const a = await store.hire(koala, "a")
  await store.hire({ id: "b", name: "Bee", agent: "claude-code" }, "b")
  // A run this app did not start still appears, unmarked.
  space.runs.set("run-foreign", { agent: "codex", status: "idle", out: [], polls: 0, pending: [] })
  space.calls.length = 0
  const roster = await store.refreshRoster()
  assert.deepEqual(space.calls, ["agentList"])
  assert.equal(store.rosterCalls, 1)
  assert.equal(roster.length, 3)
  assert.equal(roster.find((r) => r.runId === a.runId).bot.name, "Koala")
  assert.equal(roster.find((r) => r.runId === "run-foreign").bot, null)
})

test("a follow-up to an idle Bot after a roster tick is sent, not refused", async () => {
  // Regression: agent_list rows once omitted accepts_message, so the SDK
  // defaulted it to false and a roster tick marked every idle Bot as
  // refusing. The row now publishes it, as agent_status does.
  const space = new FakeSpace()
  const store = new BotStore(space)
  const t = await store.hire(koala, "open koala")
  assert.equal(await t.settle(1, 20), "idle")
  for (const status of ["idle", "crashed"]) {
    space.runs.get(t.runId).status = status
    const roster = await store.refreshRoster()
    const row = roster.find((r) => r.runId === t.runId)
    assert.equal(row.state, status)
    assert.equal(row.acceptsMessage, true, `an ${status} row accepts a message`)
    assert.equal(t.acceptsMessage, true, "the tick keeps the thread accepting")
  }
  space.calls.length = 0
  const d = await t.send("and one more thing")
  assert.deepEqual(d, { accepted: true, reason: "a new turn" })
  assert.deepEqual(space.calls, ["agentMessage"], "the follow-up reached the Space")
})

test("attachments must be verified", async () => {
  const space = new FakeSpace()
  const store = new BotStore(space)
  const r = await store.attach("/tmp/x", "inbox")
  assert.equal(r.verified, true)
  assert.equal(space.lastSend.options.targetDirectory, "inbox")
  space.sendVerified = false
  await assert.rejects(store.attach("/tmp/x", "inbox"), /not verified/)
})

test("teleport needs the approver: it sees the manifest, and undefined cancels", async () => {
  const space = new FakeSpace()
  const store = new BotStore(space)
  const { receipt, shown } = await store.teleport("firefox", (m) => ({ include: ["prefs.js"], acknowledgeSensitive: true }))
  assert.equal(shown.length, 1)
  assert.equal(shown[0].items.length, 2)
  assert.deepEqual(receipt.imported, ["firefox"])
  assert.deepEqual(space.lastDecision.include, ["prefs.js"])
  await assert.rejects(store.teleport("firefox", () => undefined), /declined/)
})

test("waitForPresence is bounded by event count and time", async () => {
  const p = await new FakeSpace().joinPresence({ id: "a", displayName: "A", agent: false })
  p._events.push({ kind: "joined", participant: { participantId: "x" } }, { kind: "left", participantId: "x" })
  const e = await waitForPresence(p, (e) => e.kind === "left", 1000, 5)
  assert.equal(e.kind, "left")
  await assert.rejects(waitForPresence(p, () => true, 50, 5), /no matching event/)
})
