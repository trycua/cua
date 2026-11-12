// `@trycua/cua/spaces/routines`, `/groups` and `/presence`: the scheduler's
// clock, persistence and firing; group bounds, fan-out and attribution; the
// presence roster fold. Pure: no native library, no Space.
import assert from "node:assert/strict"
import { test } from "node:test"

import {
  RoutineStore,
  isDue,
  isoSeconds,
  memoryStorage,
  nextFireDate,
  parseRoutines,
  routineTurnText,
  scheduleLabel,
} from "../dist/spaces/routines.js"
import { GROUP_MAX_BOTS, GroupChat, GroupChatError, GroupChatStore, canCreateGroup, frameGroupMessage } from "../dist/spaces/groups.js"
import { PRESENCE_PALETTE, PresenceRoster, agentIdentity, presenceColor, presenceTextColor, waitForPresence } from "../dist/spaces/presence.js"

const t0 = new Date(2026, 8, 25, 9, 30, 0) // local: Friday 25 September 2026, 9:30

class Runner {
  fired = []
  outcome = () => ({ kind: "started", runId: `run-${this.fired.length - 1}` })
  async fire(r) {
    this.fired.push(r)
    return this.outcome(r)
  }
}

test("schedules: next slot is strictly after the reference, in local time", () => {
  assert.equal(nextFireDate({ kind: "everyMinutes", minutes: 5 }, t0).getTime(), t0.getTime() + 300_000)
  assert.equal(nextFireDate({ kind: "everyMinutes", minutes: 0 }, t0), undefined)
  const daily = nextFireDate({ kind: "dailyAt", hour: 8, minute: 0 }, t0)
  assert.deepEqual([daily.getDate(), daily.getHours(), daily.getMinutes()], [26, 8, 0])
  const later = nextFireDate({ kind: "dailyAt", hour: 10, minute: 15 }, t0)
  assert.deepEqual([later.getDate(), later.getHours(), later.getMinutes()], [25, 10, 15])
  const exact = nextFireDate({ kind: "dailyAt", hour: 9, minute: 30 }, t0)
  assert.equal(exact.getDate(), 26, "a slot equal to the reference is not after it")
  // weekday 2 = Monday; 25 Sep 2026 is a Friday.
  const weekly = nextFireDate({ kind: "weeklyOn", weekday: 2, hour: 9, minute: 0 }, t0)
  assert.deepEqual([weekly.getDay(), weekly.getDate()], [1, 28])
  const sameDayLater = nextFireDate({ kind: "weeklyOn", weekday: 6, hour: 17, minute: 0 }, t0)
  assert.deepEqual([sameDayLater.getDay(), sameDayLater.getDate(), sameDayLater.getHours()], [5, 25, 17])
})

test("labels match the other SDKs", () => {
  assert.equal(scheduleLabel({ kind: "everyMinutes", minutes: 1 }), "Every minute")
  assert.equal(scheduleLabel({ kind: "everyMinutes", minutes: 15 }), "Every 15 minutes")
  assert.equal(scheduleLabel({ kind: "everyMinutes", minutes: 60 }), "Every hour")
  assert.equal(scheduleLabel({ kind: "everyMinutes", minutes: 120 }), "Every 2 hours")
  assert.equal(scheduleLabel({ kind: "dailyAt", hour: 0, minute: 5 }), "Every day at 12:05 AM")
  assert.equal(scheduleLabel({ kind: "weeklyOn", weekday: 2, hour: 13, minute: 0 }), "Every Monday at 1:00 PM")
  assert.equal(routineTurnText({ title: "Sweep", prompt: "Triage" }), "[routine] Sweep: Triage")
})

test("a routine is due once its first slot passes, measured from the last firing", () => {
  const r = { id: "a", botID: "b", title: "t", prompt: "p", schedule: { kind: "everyMinutes", minutes: 1 }, isEnabled: true, createdAt: isoSeconds(t0) }
  assert.equal(isDue(r, new Date(t0.getTime() + 59_000)), false)
  assert.equal(isDue(r, new Date(t0.getTime() + 60_000)), true)
  const fired = { ...r, lastFiredAt: isoSeconds(new Date(t0.getTime() + 600_000)) }
  assert.equal(isDue(fired, new Date(t0.getTime() + 630_000)), false)
  assert.equal(isDue({ ...r, isEnabled: false }, new Date(t0.getTime() + 3_600_000)), false)
})

test("the store fires what is due, once, and persists the firing", async () => {
  const storage = memoryStorage()
  const runner = new Runner()
  const store = new RoutineStore(storage, runner)
  const r = store.create({ botID: "inbox", title: "Sweep", prompt: "Triage", schedule: { kind: "everyMinutes", minutes: 1 }, now: t0 })
  assert.deepEqual(await store.tick(new Date(t0.getTime() + 30_000)), [])
  const wake = new Date(t0.getTime() + 12 * 3_600_000) // asleep overnight
  const fired = await store.tick(wake)
  assert.equal(fired.length, 1, "a backlog fires once, not once per missed slot")
  assert.equal(runner.fired[0].id, r.id)
  assert.deepEqual(await store.tick(wake), [])
  const reloaded = new RoutineStore(storage)
  const saved = reloaded.routine(r.id)
  assert.equal(saved.lastFiredAt, isoSeconds(wake))
  assert.equal(saved.lastRunID, "run-0")
  assert.equal(saved.lastOutcome, "started run run-0")
  assert.equal(reloaded.due(wake).length, 0)
})

test("a refusal uses the slot and is logged; no runner is a failure", async () => {
  const runner = new Runner()
  runner.outcome = () => ({ kind: "refused", reason: "mid-turn" })
  const store = new RoutineStore(memoryStorage(), runner)
  const r = store.create({ botID: "b", title: "T", prompt: "p", schedule: { kind: "everyMinutes", minutes: 1 }, now: t0 })
  const at = new Date(t0.getTime() + 61_000)
  const [rec] = await store.tick(at)
  assert.deepEqual(rec.firing, { kind: "refused", reason: "mid-turn" })
  assert.equal(store.routine(r.id).lastOutcome, "refused: mid-turn")
  assert.equal(store.due(at).length, 0)
  const bare = new RoutineStore(memoryStorage())
  const r2 = bare.create({ botID: "b", title: "T", prompt: "p", schedule: { kind: "everyMinutes", minutes: 1 } })
  assert.equal((await bare.fire(r2)).firing.kind, "failed")
})

test("the saved shape is the portable one; corrupt input starts empty with a complaint", () => {
  const storage = memoryStorage()
  const store = new RoutineStore(storage)
  store.create({ botID: "b", title: "T", prompt: "p", schedule: { kind: "weeklyOn", weekday: 2, hour: 9, minute: 0 }, now: t0 })
  const [row] = JSON.parse(storage.value)
  assert.deepEqual(Object.keys(row).sort(), ["botID", "createdAt", "id", "isEnabled", "prompt", "schedule", "title"])
  assert.match(row.createdAt, /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/)
  // What the Swift app writes (JSONEncoder, iso8601, sorted keys).
  const swift = `[{"botID":"b","createdAt":"2026-09-25T09:30:00Z","id":"X","isEnabled":false,"lastFiredAt":"2026-09-25T10:30:00Z","prompt":"p","schedule":{"hour":8,"kind":"dailyAt","minute":0},"title":"T"}]`
  const [parsed] = parseRoutines(swift)
  assert.equal(parsed.isEnabled, false)
  assert.deepEqual(parsed.schedule, { kind: "dailyAt", hour: 8, minute: 0 })
  const bad = new RoutineStore(memoryStorage("{nope"))
  assert.equal(bad.routines.length, 0)
  assert.match(bad.log[0].firing.reason, /could not be read/)
})

test("the scheduler loop fires a due routine without being ticked by hand", async () => {
  const runner = new Runner()
  const store = new RoutineStore(memoryStorage(), runner)
  store.create({ botID: "b", title: "T", prompt: "p", schedule: { kind: "everyMinutes", minutes: 1 }, now: new Date(Date.now() - 61_000) })
  store.startScheduler(20)
  for (let i = 0; i < 100 && runner.fired.length === 0; i++) await new Promise((r) => setTimeout(r, 10))
  await new Promise((r) => setTimeout(r, 80))
  store.stopScheduler()
  assert.equal(runner.fired.length, 1)
  assert.equal(store.isSchedulerRunning, false)
})

class Messenger {
  delivered = []
  replies = new Map()
  busy = new Set()
  refuse = new Set()
  names = { ada: "Ada", bo: "Bo", cy: "Cy" }
  async deliver(text, botID) {
    this.delivered.push({ text, botID })
    return this.refuse.has(botID) ? { botID, accepted: false, reason: "mid-turn" } : { botID, accepted: true, reason: "delivered" }
  }
  async latestReply(botID) {
    return this.replies.get(botID)
  }
  isWorking(botID) {
    return this.busy.has(botID)
  }
  displayName(botID) {
    return this.names[botID] ?? botID
  }
}

test("group bounds: 2..6, deduplicated, enforced on every mutation", () => {
  assert.throws(() => new GroupChat("x", ["ada"]), (e) => e instanceof GroupChatError && e.code === "tooFewBots")
  assert.throws(() => new GroupChat("x", ["a", "b", "c", "d", "e", "f", "g"]), (e) => e.code === "tooManyBots")
  assert.throws(() => new GroupChat("x", ["ada", "ada"]), (e) => e.code === "tooFewBots")
  assert.equal(canCreateGroup(["a", "a", "b"]), true)
  assert.equal(canCreateGroup(["a"]), false)
  const store = new GroupChatStore(new Messenger())
  const full = store.create("Full", ["a", "b", "c", "d", "e", "f"])
  assert.equal(full.membershipLabel, `6 of ${GROUP_MAX_BOTS} bots`)
  assert.throws(() => store.add("g", full.id), (e) => e.code === "full")
  assert.equal(full.messages.at(-1).undelivered, true)
  assert.match(store.lastError, /6 bots is the limit/)
  const pair = store.create("Pair", ["ada", "bo"])
  assert.throws(() => store.remove("ada", pair.id), (e) => e.code === "atFloor")
  store.add("cy", pair.id)
  assert.equal(pair.messages.at(-1).text, "Cy joined, 3 of 6 bots.")
  assert.equal(store.lastError, undefined)
})

test("one message reaches every member, framed with the room; refusals are shown", async () => {
  const m = new Messenger()
  m.refuse.add("bo")
  const store = new GroupChatStore(m)
  const chat = store.create("Launch", ["ada", "bo", "cy"])
  const ds = await store.send("Status?", chat.id)
  assert.deepEqual(
    ds.map((d) => [d.botID, d.accepted]),
    [
      ["ada", true],
      ["bo", false],
      ["cy", true],
    ],
  )
  assert.equal(m.delivered[0].text, frameGroupMessage("Status?", "ada", chat, (id) => m.displayName(id)))
  assert.match(m.delivered[0].text, /^\[group:Launch\] You are in a group chat with the user and Bo, Cy\./)
  assert.equal(m.delivered[0].text.split("\n").at(-1), "Status?")
  const refused = chat.messages.find((l) => l.undelivered)
  assert.deepEqual(refused.speaker, { kind: "bot", botID: "bo" })
  assert.match(refused.text, /Did not receive that message: mid-turn/)
})

test("replies are attributed, never duplicated, and drive the typing row", async () => {
  const m = new Messenger()
  const store = new GroupChatStore(m)
  const chat = store.create("Launch", ["ada", "bo"])
  m.replies.set("ada", " shipped \n")
  m.busy.add("bo")
  const added = await store.collectReplies(chat.id)
  assert.deepEqual(added.map((l) => [l.speaker.botID, l.text]), [["ada", "shipped"]])
  assert.deepEqual(store.workingBots(chat.id), ["bo"])
  assert.equal((await store.collectReplies(chat.id)).length, 0)
  m.replies.set("ada", "and tested")
  assert.equal((await store.collectReplies(chat.id)).length, 1)
  store.react("+1", chat.id)
  assert.equal(chat.messages.at(-1).reaction, "+1")
})

const P = (id, kind = "human") => ({ participantId: id, principalId: `u-${id}`, displayName: id.toUpperCase(), color: "", kind })

test("presence roster: join, cursor, leave; others excludes me", () => {
  const r = PresenceRoster.from(P("me"), [{ participant: P("me") }, { participant: P("bot", "agent"), cursor: { displayId: "", x: 0.1, y: 0.2, visible: true } }])
  assert.deepEqual(r.others.map((e) => [e.participantId, e.agent, e.cursor?.x]), [["bot", true, 0.1]])
  assert.equal(r.entries[0].participantId, "me")
  assert.equal(r.apply({ kind: "joined", participant: P("koala", "agent") }), true)
  assert.equal(r.apply({ kind: "joined", participant: P("koala", "agent") }), false)
  assert.equal(r.apply({ kind: "cursor_moved", participantId: "koala", cursor: { displayId: "", x: 0.25, y: 0.75, visible: true } }), true)
  assert.deepEqual(r.get("koala").cursor, { x: 0.25, y: 0.75, visible: true })
  assert.equal(r.get("koala").color, "#3b82f6")
  assert.equal(r.apply({ kind: "cursor_moved", participantId: "ghost", cursor: { displayId: "", x: 0, y: 0, visible: true } }), false)
  assert.equal(r.apply({ kind: "keep_alive" }), false)
  assert.equal(r.apply({ kind: "left", participantId: "koala" }), true)
  assert.equal(r.get("koala"), undefined)
})

test("waitForPresence folds into the roster and is bounded", async () => {
  const queue = [{ kind: "joined", participant: P("k") }, { kind: "cursor_moved", participantId: "k", cursor: { displayId: "", x: 0.5, y: 0.5, visible: true } }]
  const session = { nextEvent: async () => queue.shift() }
  const r = PresenceRoster.from(P("me"))
  const e = await waitForPresence(session, (e) => e.kind === "cursor_moved", 1_000, 10, r)
  assert.equal(e.participantId, "k")
  assert.equal(r.get("k").cursor.x, 0.5)
  await assert.rejects(waitForPresence(session, () => true, 50, 3), /no matching event/)
})

test("presence colors: stable per id, pinned to the Rust core's values, readable text", async () => {
  assert.deepEqual(["ada", "bo", "koala", "inbox", "sales"].map(presenceColor), ["#bcf60c", "#4363d8", "#46f0f0", "#bcf60c", "#f58231"])
  assert.ok(PRESENCE_PALETTE.includes(presenceColor("ünïcode-bot")))
  assert.equal(presenceTextColor("#000075"), "#ffffff")
  assert.equal(presenceTextColor("#bcf60c"), "#000000")
  assert.equal(presenceTextColor("#800000"), "#ffffff")
  assert.equal(presenceTextColor("nope"), "#000000")
  assert.deepEqual(agentIdentity("ada", "Ada"), { id: "ada", displayName: "Ada", color: "#bcf60c", agent: true })
  // The native binding (the Rust core) agrees, when it is staged.
  let native
  try {
    native = await import("../dist/native/index.js")
  } catch {
    return
  }
  for (const id of ["ada", "bo", "koala", "openkoalabots-routine-1", "ünïcode-bot", ""]) {
    assert.equal(presenceColor(id), native.presenceColor(id), id)
    assert.equal(presenceTextColor(presenceColor(id)), native.presenceTextColor(presenceColor(id)), id)
  }
})

test("the roster's assigned color wins over the stable one, for avatar and cursor alike", () => {
  const stable = presenceColor("koala")
  const me = { participantId: "me", principalId: "operator", displayName: "Op", color: stable, kind: "human" }
  const r = PresenceRoster.from(me)
  assert.equal(r.colorOf("koala"), stable, "before it joins: the stable color")
  // The operator holds Koala's stable color, so the server assigned another.
  r.apply({ kind: "joined", participant: { participantId: "k", principalId: "koala", displayName: "Koala", color: "#123456", kind: "agent" } })
  assert.equal(r.colorOf("koala"), "#123456")
  assert.equal(r.get("k").color, r.colorOf("koala"))
  r.apply({ kind: "left", participantId: "k" })
  assert.equal(r.colorOf("koala"), stable, "after it leaves: the stable color again")
})
