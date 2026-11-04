// Routines and group chats over the app's BotStore: the live RoutineRunner
// and GroupMessenger (the Swift sample's semantics), the reply extraction
// from a real ACP output tail, and the server's routes. Fakes only.
import assert from "node:assert/strict"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { after, before, test } from "node:test"
import { GroupChatStore } from "@trycua/cua/spaces/groups"
import { RoutineStore } from "@trycua/cua/spaces/routines"
import { BotStore, agentOptions } from "../dist/core/app.js"
import { BotStoreGroupMessenger, BotStoreRoutineRunner, jsonStorage, replyText } from "../dist/core/coworkers.js"
import { startServer } from "../dist/server/app.js"
import { FakeSpace, FakeSpaces } from "./fake.mjs"

const koala = { id: "koala", name: "Koala", agent: "claude-code" }
const settle = (t) => t.settle(1, 20)

test("agent options: none without an endpoint, every field with one", () => {
  assert.equal(agentOptions(undefined), undefined)
  assert.equal(agentOptions({}), undefined)
  assert.deepEqual(agentOptions({ baseUrl: "http://m:8787", model: "claude-mock-1", envFromHost: ["ANTHROPIC_API_KEY"] }), {
    envFromHost: ["ANTHROPIC_API_KEY"], env: new Map(), repo: undefined, branch: undefined, cwd: undefined, model: "claude-mock-1", baseUrl: "http://m:8787", exitWhenIdle: undefined,
  })
})

test("the store passes its endpoint to agentStart and finds a Bot's thread", async () => {
  const space = new FakeSpace()
  const seen = []
  const start = space.agentStart.bind(space)
  space.agentStart = (a, p, s, o) => (seen.push(o), start(a, p, s, o))
  const store = new BotStore(space, { baseUrl: "http://m", model: "x" })
  const t = await store.hire(koala, "hi")
  assert.equal(seen[0].baseUrl, "http://m")
  assert.equal(store.threadFor("koala"), t)
  assert.equal(store.bot("koala").name, "Koala")
  assert.equal(store.threadFor("nobody"), undefined)
})

test("replyText is the turn's messages: no install, echo or turn-end", async () => {
  const space = new FakeSpace()
  const store = new BotStore(space)
  const t = await store.hire(koala, "Hello. mock: say probe-ok")
  space.runs.get(t.runId).pending = [
    { kind: "install", text: "node check 24.21.0" },
    { kind: "user_message", text: "Hello. mock: say probe-ok" },
    "probe-ok",
  ]
  await settle(t)
  assert.equal(replyText(t), "probe-ok")
})

test("the routine runner hires an unhired Bot, sends to an idle one, refuses a busy one", async () => {
  const space = new FakeSpace()
  const store = new BotStore(space)
  store.register(koala)
  const runner = new BotStoreRoutineRunner(store)
  const r = { id: "r1", botID: "koala", title: "Sweep", prompt: "triage", schedule: { kind: "everyMinutes", minutes: 1 }, isEnabled: true, createdAt: "2026-09-25T00:00:00Z" }
  const first = await runner.fire(r)
  assert.equal(first.kind, "started")
  const t = store.thread(first.runId)
  assert.equal(t.turns[0].message, "[routine] Sweep: triage")
  const busy = await runner.fire(r)
  assert.equal(busy.kind, "refused")
  assert.match(busy.reason, /mid-turn/)
  await settle(t)
  const again = await runner.fire(r)
  assert.deepEqual(again, { kind: "started", runId: first.runId }, "a new turn on the same thread")
  assert.equal(t.turns.length, 2)
  assert.equal((await runner.fire({ ...r, botID: "ghost" })).kind, "failed")
})

test("the routine store fires through the live runner and persists to a file", async () => {
  const dir = mkdtempSync(join(tmpdir(), "okb-routines-"))
  try {
    const store = new BotStore(new FakeSpace())
    store.register(koala)
    const file = join(dir, "routines.json")
    const routines = new RoutineStore(jsonStorage(file), new BotStoreRoutineRunner(store))
    const created = new Date(Date.now() - 61_000)
    const r = routines.create({ botID: "koala", title: "T", prompt: "p", schedule: { kind: "everyMinutes", minutes: 1 }, now: created })
    const [rec] = await routines.tick(new Date())
    assert.equal(rec.firing.kind, "started")
    const saved = JSON.parse(readFileSync(file, "utf8"))[0]
    assert.equal(saved.lastRunID, rec.firing.runId)
    assert.equal(new RoutineStore(jsonStorage(file)).routine(r.id).lastOutcome, `started run ${rec.firing.runId}`)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test("the group messenger hires members, frames them, and attributes replies once", async () => {
  const space = new FakeSpace()
  const store = new BotStore(space)
  store.register({ id: "ada", name: "Ada", agent: "claude-code" })
  store.register({ id: "bo", name: "Bo", agent: "claude-code" })
  const groups = new GroupChatStore(new BotStoreGroupMessenger(store))
  const chat = groups.create("Standup", ["ada", "bo"])
  const ds = await groups.send("Status?", chat.id)
  assert.deepEqual(ds.map((d) => [d.botID, d.accepted, d.reason]), [["ada", true, "started for this group"], ["bo", true, "started for this group"]])
  assert.match(store.threadFor("ada").turns[0].message, /^\[group:Standup\] You are in a group chat with the user and Bo\./)
  assert.deepEqual(groups.workingBots(chat.id), ["ada", "bo"])
  assert.equal((await groups.collectReplies(chat.id)).length, 0, "no reply while working")
  for (const id of ["ada", "bo"]) await settle(store.threadFor(id))
  const added = await groups.collectReplies(chat.id)
  assert.deepEqual(added.map((l) => l.speaker.botID), ["ada", "bo"])
  assert.match(added[0].text, /^did: \[group:Standup\]/)
  assert.equal((await groups.collectReplies(chat.id)).length, 0)
  // A turn the group did not ask for (a routine, a direct message) stays out of the group.
  const ada = store.threadFor("ada")
  assert.equal((await ada.send("[routine] Notes: draft")).accepted, true)
  await settle(ada)
  assert.equal((await groups.collectReplies(chat.id)).length, 0)
  // A busy member refuses and the group shows it.
  const again = await groups.send("More?", chat.id)
  assert.ok(again.every((d) => d.accepted))
  const third = await groups.send("And?", chat.id)
  assert.ok(third.every((d) => !d.accepted))
  assert.ok(chat.messages.some((l) => l.undelivered && l.speaker.botID === "ada"))
})

let srv
let dataDir
before(async () => {
  dataDir = mkdtempSync(join(tmpdir(), "okb-server-"))
  srv = await startServer({ spaces: new FakeSpaces(), stream: () => ({ openStream: async () => ({}), closeStream: async () => {} }) }, { pollMs: 20, routineTickMs: 20, dataDir })
})
after(async () => {
  await srv.close()
  rmSync(dataDir, { recursive: true, force: true })
})
const call = (path, body) =>
  fetch(`${srv.url}/api/${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: { authorization: `Bearer ${srv.token}`, "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  }).then(async (r) => ({ status: r.status, body: await r.json() }))

test("server: routines and group chats end to end over the routes", async () => {
  await call("spaces/add", { url: "10.0.0.9:3211", token: "t", name: "dev" })
  const a = (await call("bots", { name: "Ada", prompt: "hello" })).body
  const b = (await call("bots", { name: "Bo", prompt: "hello" })).body
  assert.equal((await call("routines", { botID: "nobody", title: "x", prompt: "y", schedule: { kind: "everyMinutes", minutes: 1 } })).status, 400)
  assert.equal((await call("routines", { botID: a.botId, title: "x", prompt: "y", schedule: { kind: "hourly" } })).status, 400)
  const r = (await call("routines", { botID: a.botId, title: "Sweep", prompt: "triage", schedule: { kind: "dailyAt", hour: 8, minute: 0 } })).body
  assert.equal(r.isEnabled, true)
  assert.equal((await call(`routines/${r.id}`, { enabled: false })).body.isEnabled, false)
  const fired = (await call(`routines/${r.id}/run`, {})).body
  assert.ok(["started", "refused"].includes(fired.firing.kind))
  let state = (await call("state")).body
  assert.equal(state.routines[0].label, "Every day at 8:00 AM")
  assert.ok(state.routines[0].lastFiredAt)
  assert.deepEqual(JSON.parse(readFileSync(join(dataDir, "routines.json"), "utf8")).map((x) => x.id), [r.id])
  assert.equal((await call("groups", { members: [a.botId] })).status, 400)
  const g = (await call("groups", { title: "Standup", members: [a.botId, b.botId] })).body
  assert.equal(g.membershipLabel, "2 of 6 bots")
  assert.equal((await call(`groups/${g.id}/remove`, { botID: a.botId })).status, 400)
  for (let i = 0; i < 50; i++) {
    state = (await call("state")).body
    if (state.bots.every((x) => x.state === "idle")) break
    await new Promise((res) => setTimeout(res, 20))
  }
  const sent = (await call(`groups/${g.id}/send`, { text: "Status?" })).body
  assert.equal(sent.deliveries.length, 2)
  for (let i = 0; i < 100; i++) {
    state = (await call("state")).body
    if (state.groups[0].messages.filter((l) => l.speaker.kind === "bot").length >= 2) break
    await new Promise((res) => setTimeout(res, 20))
  }
  const bots = state.groups[0].messages.filter((l) => l.speaker.kind === "bot").map((l) => l.speaker.botID)
  assert.deepEqual(bots.sort(), [a.botId, b.botId].sort())
  assert.equal((await call(`routines/${r.id}/delete`, {})).body.deleted, true)
  assert.deepEqual(JSON.parse(readFileSync(join(dataDir, "bots.json"), "utf8")).map((x) => x.name), ["Ada", "Bo"])
})
