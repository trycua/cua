import assert from "node:assert/strict"
import { after, before, test } from "node:test"
import { startServer } from "../dist/server/app.js"
import { FakeSpaces } from "./fake.mjs"

let srv
let spaces
const opened = []
const created = []
before(async () => {
  spaces = new FakeSpaces()
  srv = await startServer(
    {
      spaces,
      stream: () => ({
        openStream: async (o) => (opened.push(o), { wsUrl: "ws://h/media?ticket=t", ticket: "t", mediaSessionId: "m1", codec: "h264", width: 1, height: 1, needsHeaders: false }),
        closeStream: async (id) => void opened.push({ closed: id }),
        windows: async () => [{ windowId: "0x1a00003", app: "Firefox", title: "Mozilla Firefox", width: 1200, height: 800 }],
      }),
      create: async (call) => {
        created.push(call)
        return spaces.add(`created-${call.name}:3211`, undefined, call.name)
      },
      cloud: false,
    },
    { pollMs: 20 },
  )
})
after(() => srv.close())

const call = (path, body, token = srv.token) =>
  fetch(`${srv.url}/api/${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  }).then(async (r) => ({ status: r.status, body: await r.json() }))

test("binds loopback and requires the bearer", async () => {
  assert.match(srv.url, /^http:\/\/127\.0\.0\.1:\d+$/)
  assert.equal((await call("state", undefined, "wrong")).status, 401)
  assert.equal((await call("state")).status, 200)
})

test("add a Space, hire a Bot, message it, and watch the roster", async () => {
  assert.equal((await call("bots", { prompt: "x" })).status, 409, "no Space selected yet")
  const added = await call("spaces/add", { url: "10.0.0.5:3211", token: "t", name: "dev" })
  assert.equal(added.status, 200)
  const { body: hired } = await call("bots", { name: "Koala", prompt: "open koala" })
  assert.ok(hired.runId)
  // Refused while running (fake runs settle on their second poll).
  const refused = await call(`bots/${hired.runId}/message`, { text: "now" })
  assert.equal(refused.body.accepted, false)
  let state
  for (let i = 0; i < 50; i++) {
    state = (await call("state")).body
    if (state.bots[0]?.state === "idle") break
    await new Promise((r) => setTimeout(r, 20))
  }
  assert.equal(state.bots[0].state, "idle")
  assert.ok(state.bots[0].items.some((i) => i.kind === "message" && i.text === "did: open koala"))
  assert.equal(state.bots[0].preview, "did: open koala")
  assert.equal(state.roster[0].bot.name, "Koala")
  assert.equal((await call(`bots/${hired.runId}/message`, { text: "again" })).body.accepted, true)
})

test("files, teleport with a seen manifest, stream tickets, presence", async () => {
  const up = await fetch(`${srv.url}/api/files?name=${encodeURIComponent("../../evil.txt")}`, {
    method: "POST", headers: { authorization: `Bearer ${srv.token}` }, body: "hello",
  }).then((r) => r.json())
  assert.equal(up.verified, true)
  assert.equal((await call("teleport", { include: [] })).status, 400, "no manifest approved")
  const { body: m } = await call("teleport/manifest", { app: "firefox" })
  assert.equal(m.manifest.items.length, 2)
  const tp = await call("teleport", { manifestId: m.manifestId, include: ["prefs.js"], acknowledgeSensitive: true })
  assert.deepEqual(tp.body.imported, ["firefox"])
  assert.equal((await call("teleport", { manifestId: m.manifestId })).status, 400, "a manifest approves one teleport")
  const t = await call("stream", { maxFps: 10 })
  assert.equal(t.body.ticket, "t")
  assert.equal(opened[0].maxFps, 10)
  await call("stream/close", { mediaSessionId: "m1" })
  assert.deepEqual(opened.at(-1), { closed: "m1" })
  const j = await call("presence/join", { displayName: "Me" })
  assert.equal(j.body.me.displayName, "Me")
  assert.equal((await call("presence/cursor", { x: 0.5, y: 0.5 })).status, 200)
  assert.equal((await call("presence/leave", {})).status, 200)
})

test("the event socket needs the token", async () => {
  const bad = new WebSocket(`${srv.url.replace("http", "ws")}/events?token=nope`)
  await new Promise((r) => { bad.onerror = r; bad.onclose = r })
  const ws = new WebSocket(`${srv.url.replace("http", "ws")}/events?token=${srv.token}`)
  const first = await new Promise((r, j) => { ws.onmessage = (e) => r(JSON.parse(e.data)); ws.onerror = j })
  assert.equal(first.type, "state")
  ws.close()
})

test("delete forgets the selection", async () => {
  const { body } = await call("spaces")
  const id = body.selected
  assert.equal((await call("spaces/delete", { id })).status, 200)
  assert.deepEqual(spaces.deleted, [id])
  assert.equal((await call("state")).body.space, null)
})

test("the wizard's plan becomes one create call, validated first", async () => {
  const bad = await call("spaces/create", { plan: { image: "ghcr.io/trycua/macos:26", target: "cloud", name: "mac" } })
  assert.equal(bad.status, 400)
  assert.match(bad.body.error, /does not run in Cua Cloud/)
  const noCloud = await call("spaces/create", { plan: { image: "ghcr.io/trycua/linux:24.04", target: "cloud", name: "desk" } })
  assert.equal(noCloud.status, 501, "Cua Cloud needs credentials")
  assert.equal(created.length, 0, "nothing created for refused plans")
  const ok = await call("spaces/create", { plan: { image: "ghcr.io/trycua/linux:24.04", target: "local", name: "desk", cpus: 2, memory_mb: 4096 } })
  assert.equal(ok.status, 200)
  assert.deepEqual(created, [{ on: "local", image: "ghcr.io/trycua/linux:24.04", kind: "container", runtime: "auto", name: "desk", cpus: 2, memoryMb: 4096, wait: true, spacesd: true }])
  assert.equal((await call("spaces")).body.selected, ok.body.id, "the new Space is selected")
  assert.deepEqual((await call("config")).body, { cloud: false, create: true })
})

test("window list and per-window stream tickets, for picture in picture", async () => {
  await call("spaces/add", { url: "10.0.0.9:3211", token: "t", name: "wins" })
  const { body: sel } = await call("spaces")
  const space = await spaces.space(sel.selected)
  // No window_stream: an empty list, and a window ticket is refused.
  assert.deepEqual((await call("windows")).body, { windows: [] })
  assert.equal((await call("stream", { windowId: "0x1a00003" })).status, 409)
  space._info.features.push("window_stream")
  assert.deepEqual((await call("windows")).body.windows, [{ windowId: "0x1a00003", app: "Firefox", title: "Mozilla Firefox", width: 1200, height: 800 }])
  opened.length = 0
  assert.equal((await call("stream", { windowId: "0x1a00003", maxFps: 15 })).status, 200)
  assert.deepEqual(opened[0], { maxFps: 15, maxDimension: 1280, windowId: "0x1a00003" })
  assert.equal((await call("stream", {})).status, 200, "the desktop still streams")
  assert.equal(opened[1].windowId, undefined)
})
