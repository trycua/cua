// Teleport on the MIT SDK. "Teleport an app…" (catalog, plan, run, window
// drags) ships with Cua Spaces, so its routes only say so. Session teleport
// is a tool call: the embedded runtime refuses it with HostCapabilityMissing
// ("teleport ships with Cua Spaces"), and the server and the scenario runner
// report that refusal instead of a raw error. No host app, window or Space
// is touched.
import assert from "node:assert/strict"
import { after, before, test } from "node:test"
import { startServer } from "../dist/server/app.js"
import { APP_TELEPORT_MESSAGE, teleportNeedsCuaSpaces } from "../dist/core/teleport.js"
import { FakeSpaces } from "./fake.mjs"

// What the embedded runtime throws (cua_spaces::Error::needs_cua_spaces, as
// the native binding's CuaError.HostCapabilityMissing).
const REFUSAL =
  "teleport is not available on this host: teleport ships with Cua Spaces (source-available, FSL-1.1-MIT); " +
  "connect to the daemon Cua Spaces runs (`cua daemon` from the Cua Spaces app) or register the Cua Spaces extensions in this process"
const embeddedRefusal = () => Object.assign(new Error(REFUSAL), { tag: "HostCapabilityMissing" })

let srv
let spaces
before(async () => {
  spaces = new FakeSpaces()
  srv = await startServer({ spaces, stream: () => ({ openStream: async () => ({}), closeStream: async () => {} }) }, { pollMs: 50 })
})
after(() => srv.close())

const call = (path, body) =>
  fetch(`${srv.url}/api/${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: { authorization: `Bearer ${srv.token}`, "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  }).then(async (r) => ({ status: r.status, body: await r.json() }))

test("the app teleport routes say it ships with Cua Spaces", async () => {
  assert.equal(APP_TELEPORT_MESSAGE, "App teleport ships with Cua Spaces (source-available).")
  const answers = [
    await call("teleport/apps"),
    await call("teleport/icon", { id: "vscode" }),
    await call("teleport/plan", { id: "vscode", options: { moves: "app_only", files: [] } }),
    await call("teleport/run", { json: "{}", approved: true }),
    await call("teleport/window-drags", { enabled: true }),
  ]
  for (const a of answers) assert.deepEqual(a, { status: 501, body: { error: APP_TELEPORT_MESSAGE } })
  assert.deepEqual((await call("config")).body, { cloud: false, create: false })
})

test("session teleport on the embedded runtime: the refusal says it ships with Cua Spaces", async () => {
  await call("spaces/add", { url: "10.0.0.5:3211", token: "t", name: "dev" })
  const space = await spaces.space("space://direct/10.0.0.5:3211")
  space.teleportManifest = async () => {
    throw embeddedRefusal()
  }
  const m = await call("teleport/manifest", { app: "firefox" })
  assert.equal(m.status, 501)
  assert.equal(m.body.error, REFUSAL)
  // Nothing was shown, so nothing can be approved.
  assert.equal((await call("teleport", { include: [] })).status, 400)
})

test("only a HostCapabilityMissing that names Cua Spaces counts as the refusal", () => {
  assert.equal(teleportNeedsCuaSpaces(embeddedRefusal()), REFUSAL)
  // The SDK's wrapped form (SpacesError code) and the raw message.
  assert.equal(teleportNeedsCuaSpaces(Object.assign(new Error(REFUSAL), { code: "host_capability_missing" })), REFUSAL)
  assert.equal(teleportNeedsCuaSpaces(new Error(`Error: ${REFUSAL}`)), REFUSAL)
  // Other refusals stay errors: a declined approval, a missing local runtime,
  // the Keyvault asking for the Cua app.
  assert.equal(teleportNeedsCuaSpaces(Object.assign(new Error("the approver declined"), { tag: "TeleportRefused" })), undefined)
  assert.equal(teleportNeedsCuaSpaces(Object.assign(new Error("local runtime is not available on this host: install Docker"), { tag: "HostCapabilityMissing" })), undefined)
  assert.equal(teleportNeedsCuaSpaces(new Error("teleport refused: requires_cua_app: teleport goes through the Cua Keyvault")), undefined)
})
