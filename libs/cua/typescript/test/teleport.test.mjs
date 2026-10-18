// `@trycua/cua/teleport`: the headless "Teleport an app…" state machine,
// drag payloads (the same cases as the Rust parser), window drops, the core
// JSON converters and the rendered markup. No native library, no Space, no
// host apps: every catalog and plan here is a fixture.
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"

import {
  CUA_INSTALL_URL,
  TeleportPickerController,
  canConfirm,
  requiresCuaApp,
  classifyBrowserDrop,
  entryFromCore,
  idleWindowDrop,
  initialState,
  parseDrop,
  planFromCore,
  pngDataUrl,
  progress,
  reduce,
  reduceWindowDrop,
  renderDropZone,
  renderPicker,
  runEventFromCore,
  runReportFromCore,
  sections,
  browserViewportOrigin,
  screenToClient,
} from "../dist/teleport/index.js"

const here = dirname(fileURLToPath(import.meta.url))
const drops = JSON.parse(
  readFileSync(join(here, "../../crates/cua-teleport/src/ux/testdata/drops.json"), "utf8"),
)

// Core JSON as `cua_teleport::ux` serializes it (snake_case).
const coreVscode = {
  id: "vscode",
  name: "Visual Studio Code",
  host_path: "/fixture/Visual Studio Code.app",
  host_app_id: "com.microsoft.VSCode",
  version: "1.0.0",
  icon: null,
  capability: "install_only",
  reason: null,
  moves: ["app_only", "app_with_files"],
  provider_id: null,
  install: { kind: "manifest", id: "vscode", version: "1.139.0", license: "MIT", arches: ["aarch64", "x86_64"] },
  launch: { bin: "code", args: ["--no-sandbox"], terminal: false },
  last_used_ms: null,
}
const vscode = entryFromCore(coreVscode)
const firefox = entryFromCore({
  ...coreVscode,
  id: "firefox",
  name: "Firefox",
  host_path: "/fixture/Firefox.app",
  host_app_id: "org.mozilla.firefox",
  capability: "full",
  moves: ["app_only", "app_with_files", "app_with_state"],
  provider_id: "firefox",
  install: { kind: "image" },
  launch: { bin: "firefox", args: [], terminal: false },
  last_used_ms: 5,
})
const safari = entryFromCore({
  ...coreVscode,
  id: "com.apple.Safari",
  name: "Safari",
  host_path: "/fixture/Safari.app",
  host_app_id: "com.apple.Safari",
  capability: "unsupported",
  reason: "no Linux build in the install manifest and no teleport provider for its state",
  moves: [],
  install: null,
  launch: null,
})
const catalog = [firefox, vscode, safari]

const corePlan = {
  app: coreVscode,
  space_id: "space://direct/127.0.0.1:3211",
  moves: "app_with_files",
  steps: [
    { kind: "install", ids: ["vscode"] },
    { kind: "send_files", paths: ["/tmp/project"], subdir: "Teleported" },
    { kind: "launch", bin: "code", args: ["--no-sandbox", "/root/Downloads/Teleported/project"], files: ["/root/Downloads/Teleported/project"], terminal: false },
  ],
  consent: [
    { kind: "install", key: "vscode", label: "Install Visual Studio Code 1.139.0", detail: "pinned, verified by sha256 7a8d", bytes: 0, sensitive: false },
    { kind: "folder", key: "/tmp/project", label: "/tmp/project", detail: "2 files, to /root/Downloads/Teleported/project", bytes: 18, sensitive: false },
  ],
  sensitive: false,
  total_bytes: 18,
  warnings: [],
}

test("drag payloads parse the same as the Rust core", () => {
  for (const c of drops.cases) {
    const p = parseDrop(c.items)
    assert.equal(p.kind, c.kind, JSON.stringify(c.items))
    for (const k of ["apps", "files", "urls", "ignored"]) assert.deepEqual(p[k], c[k], `${k} of ${JSON.stringify(c.items)}`)
  }
})

test("browser drops find the app by URI, then by file name", () => {
  const byUri = classifyBrowserDrop({ uriList: "file:///fixture/Visual%20Studio%20Code.app/" }, catalog)
  assert.equal(byUri.kind, "app")
  assert.equal(byUri.entry.id, "vscode")
  const byName = classifyBrowserDrop({ fileNames: ["Firefox.app"] }, catalog)
  assert.equal(byName.kind, "app")
  assert.equal(byName.entry.id, "firefox")
  const unknown = classifyBrowserDrop({ fileNames: ["Nope.app"] }, catalog)
  assert.equal(unknown.kind, "app")
  assert.equal(unknown.entry, null)
  assert.deepEqual(classifyBrowserDrop({ fileNames: ["notes.txt"] }, catalog), { kind: "files", names: ["notes.txt"], paths: [] })
  assert.deepEqual(classifyBrowserDrop({ text: "https://example.com" }, catalog), { kind: "url", urls: ["https://example.com"] })
  assert.deepEqual(classifyBrowserDrop({}, catalog), { kind: "empty" })
})

test("core JSON converts to the SDK record shape", () => {
  assert.equal(vscode.installSource, "manifest")
  assert.equal(vscode.installId, "vscode")
  assert.equal(vscode.launchBin, "code")
  assert.equal(JSON.parse(vscode.json).id, "vscode")
  const plan = planFromCore(corePlan)
  assert.deepEqual(plan.steps.map((s) => s.kind), ["install", "files", "launch"])
  assert.equal(plan.steps[1].summary, "Send 1 item(s) to ~/Downloads/Teleported")
  assert.equal(plan.steps[2].summary, "Open code with 1 item(s)")
  assert.equal(plan.totalBytes, 18)
  const ev = runEventFromCore({ step: 1, steps: 3, kind: "files", phase: "progress", detail: "/tmp/project", done_bytes: 9, total_bytes: 18 })
  assert.equal(ev.doneBytes, 9)
  const r = runReportFromCore({ app_id: "vscode", installed: ["vscode"], sent: ["/x"], imported: [], skipped: [], launched: true })
  assert.equal(r.launched, true)
})

test("the picker walks pick, options, consent, run, done", () => {
  let s = initialState("Dev Space")
  s = reduce(s, { type: "loaded", entries: catalog })
  assert.equal(s.step, "pick")
  assert.equal(s.selectedId, "firefox")
  assert.deepEqual(sections(s).map((x) => [x.title, x.entries.map((e) => e.id)]), [
    ["Recent", ["firefox"]],
    ["Apps", ["vscode"]],
    ["Not available", ["com.apple.Safari"]],
  ])
  // Search narrows and moves the selection to a visible, enabled row.
  s = reduce(s, { type: "query", query: "visual" })
  assert.equal(s.selectedId, "vscode")
  // An unsupported row cannot be chosen.
  assert.equal(reduce(s, { type: "choose", id: "com.apple.Safari" }), s)
  s = reduce(s, { type: "choose" })
  assert.equal(s.step, "options")
  assert.equal(s.move, "app_only")
  s = reduce(s, { type: "move", move: "app_with_files" })
  assert.equal(reduce(s, { type: "plan" }).step, "options", "files are required")
  s = reduce(s, { type: "files", files: ["/tmp/project", "/tmp/project"] })
  assert.deepEqual(s.files, ["/tmp/project"])
  assert.equal(reduce(s, { type: "move", move: "app_with_state" }).move, "app_with_files", "not offered")
  s = reduce(s, { type: "plan" })
  assert.equal(s.step, "planning")
  s = reduce(s, { type: "planned", plan: planFromCore(corePlan) })
  assert.equal(s.step, "consent")
  assert.ok(canConfirm(s))
  s = reduce(s, { type: "confirm" })
  s = reduce(s, { type: "progress", event: { step: 1, steps: 3, kind: "files", phase: "progress", detail: "", doneBytes: 9, totalBytes: 18 } })
  assert.equal(Math.round(progress(s) * 100), 50)
  s = reduce(s, { type: "finished", report: { appId: "vscode", installed: ["vscode"], sent: ["/x"], imported: [], skipped: [], launched: true } })
  assert.equal(s.step, "done")
})

test("secrets need an acknowledgement; errors go back where they came from", () => {
  let s = reduce(initialState("S"), { type: "preselect", entry: firefox })
  assert.equal(s.step, "options")
  s = reduce(s, { type: "move", move: "app_with_state" })
  s = reduce(s, { type: "plan" })
  s = reduce(s, { type: "failed", message: "boom" })
  assert.equal(s.step, "error")
  assert.equal(reduce(s, { type: "back" }).step, "options")
  s = reduce(reduce(s, { type: "back" }), { type: "plan" })
  const secret = planFromCore({ ...corePlan, sensitive: true, consent: [{ kind: "secret", key: "cookies.sqlite", label: "Cookies", detail: "", bytes: 1, sensitive: true }] })
  s = reduce(s, { type: "planned", plan: secret })
  assert.equal(canConfirm(s), false)
  assert.equal(reduce(s, { type: "confirm" }).step, "consent")
  s = reduce(s, { type: "acknowledge", value: true })
  assert.equal(reduce(s, { type: "confirm" }).step, "running")
  // Preselecting an unsupported app explains why.
  const e = reduce(initialState("S"), { type: "preselect", entry: safari })
  assert.equal(e.step, "error")
  assert.match(e.error, /no Linux build/)
  // A catalog that arrives after planning began keeps the flow.
  let p = reduce(reduce(initialState("S"), { type: "preselect", entry: vscode }), { type: "plan" })
  p = reduce(p, { type: "loaded", entries: catalog })
  assert.equal(p.step, "planning")
  assert.equal(p.entries.length, 3)
  // A drop with files defaults to "with files".
  assert.equal(reduce(initialState("S"), { type: "preselect", entry: vscode, files: ["/a"] }).move, "app_with_files")
})

test("the controller drives a host end to end", async () => {
  const calls = []
  const host = {
    catalog: async () => catalog,
    plan: async (entry, options) => {
      calls.push(["plan", entry.id, options])
      return planFromCore(corePlan)
    },
    run: async (plan, consent, onEvent) => {
      calls.push(["run", plan.app.id, consent])
      onEvent({ step: 0, steps: 3, kind: "install", phase: "started", detail: "", doneBytes: 0, totalBytes: 0 })
      return { appId: "vscode", installed: ["vscode"], sent: [], imported: [], skipped: [], launched: true }
    },
    chooseFiles: async () => ["/tmp/project"],
  }
  const c = new TeleportPickerController(host, { spaceName: "Dev" })
  let changes = 0
  const stop = c.subscribe(() => changes++)
  await c.load()
  c.choose("vscode")
  c.dispatch({ type: "move", move: "app_with_files" })
  await c.chooseFiles()
  await c.plan()
  assert.equal(c.state.step, "consent")
  await c.confirm()
  assert.equal(c.state.step, "done")
  assert.deepEqual(calls[0], ["plan", "vscode", { moves: "app_with_files", files: ["/tmp/project"] }])
  assert.deepEqual(calls[1], ["run", "vscode", { approved: true, acknowledgeSensitive: false, saveToKeyvault: false, acknowledgeRelayPlaintext: false }])
  assert.ok(changes >= 6)
  stop()
  // A failing catalog surfaces an error.
  const bad = new TeleportPickerController({ ...host, catalog: async () => { throw new Error("no apps") } }, { spaceName: "Dev" })
  await bad.load()
  assert.equal(bad.state.step, "error")
  assert.equal(bad.state.error, "no apps")
})

test("a Keyvault requires_cua_app refusal becomes the Install Cua prompt", async () => {
  // Every shape the refusal reaches a client in: the SDK error text, the
  // teleport_app tool result, and the Keyvault's own RequiresCuaApp record.
  const shapes = [
    new Error("teleport refused: requires_cua_app: teleport goes through the Cua Keyvault, which needs the Cua app"),
    { moved: false, error: { code: "requires_cua_app", message: "no Keyvault" } },
    { installed: false, install_url: "https://cua.ai/download", open_url: "cua://keyvault", message: "Teleport requires the Cua app" },
  ]
  for (const shape of shapes) {
    const p = requiresCuaApp(shape)
    assert.ok(p, JSON.stringify(shape))
    assert.equal(p.title, "Install Cua to teleport your session")
    assert.match(p.message, /keeps your logins in its Keyvault and asks you before sharing them/)
    assert.equal(p.url, CUA_INSTALL_URL)
  }
  // Installed but not running: open it instead.
  const open = requiresCuaApp("requires_cua_app: Cua is installed but not running")
  assert.equal(open.installed, true)
  assert.equal(open.url, "cua://keyvault")
  // Other failures stay raw; the link never comes from the error.
  assert.equal(requiresCuaApp(new Error("denied")), null)
  assert.equal(requiresCuaApp({ error: { code: "disabled" } }), null)
  assert.equal(requiresCuaApp({ code: "requires_cua_app", install_url: "https://evil.example" }).url, CUA_INSTALL_URL)

  // The controller shows the prompt instead of the raw error, and back clears it.
  const host = {
    catalog: async () => catalog,
    plan: async () => planFromCore({ ...corePlan, moves: "app_only", steps: [corePlan.steps[0]], consent: [corePlan.consent[0]] }),
    run: async () => {
      throw Object.assign(new Error("requires_cua_app: teleport goes through the Cua Keyvault"), { tag: "TeleportRefused" })
    },
  }
  const c = new TeleportPickerController(host, { spaceName: "Dev" })
  await c.load()
  c.choose("firefox")
  await c.plan()
  await c.confirm()
  assert.equal(c.state.step, "error")
  assert.equal(c.state.installPrompt?.title, "Install Cua to teleport your session")
  const html = renderPicker(c, new Map())
  assert.match(html, /Install Cua to teleport your session/)
  assert.match(html, /href="https:\/\/cua\.ai\/install"/)
  assert.doesNotMatch(html, /requires_cua_app/)
  c.dispatch({ type: "back" })
  assert.equal(c.state.step, "consent")
  assert.equal(c.state.installPrompt, null)
  // Any other failure keeps the raw message and no prompt.
  const other = reduce(initialState("S"), { type: "failed", message: "boom" })
  assert.equal(other.installPrompt, null)
  assert.match(renderPicker({ state: other }, new Map()), /boom/)
})

test("window drops show the preview and commit over a Space", () => {
  const win = { windowId: 7, appName: "Visual Studio Code", title: "main.rs" }
  let r = reduceWindowDrop(idleWindowDrop, { phase: "start", x: 0, y: 0, window: win, app: vscode })
  assert.equal(r.capture, 7)
  assert.ok(r.state.active)
  r = reduceWindowDrop(r.state, { phase: "thumbnail", url: "data:image/png;base64,AA==" })
  r = reduceWindowDrop(r.state, { phase: "over", id: "space://direct/1" })
  const html = renderDropZone(r.state, "Dev")
  assert.match(html, /data:image\/png;base64,AA==/)
  assert.match(html, /Release to teleport to Dev/)
  const end = reduceWindowDrop(r.state, { phase: "end", x: 1, y: 1 })
  assert.equal(end.commit.targetId, "space://direct/1")
  assert.equal(end.commit.app.id, "vscode")
  assert.equal(end.state.active, false)
  // Unsupported apps and releases outside a Space commit nothing.
  assert.equal(reduceWindowDrop(idleWindowDrop, { phase: "start", x: 0, y: 0, window: win, app: safari }).state.active, false)
  const out = reduceWindowDrop(reduceWindowDrop(idleWindowDrop, { phase: "start", x: 0, y: 0, window: win, app: vscode }).state, { phase: "end", x: 0, y: 0 })
  assert.equal(out.commit, null)
  assert.deepEqual(screenToClient(110, 220, { x: 10, y: 20 }), { x: 100, y: 200 })
  assert.deepEqual(browserViewportOrigin({ screenX: 0, screenY: 0, outerWidth: 1000, innerWidth: 1000, outerHeight: 900, innerHeight: 800 }), { x: 0, y: 100 })
  assert.equal(pngDataUrl(new Uint8Array([1, 2, 3])), "data:image/png;base64,AQID")
})

test("the rendered picker escapes names and disables unsupported apps", () => {
  const evil = entryFromCore({ ...coreVscode, id: "x", name: "<img onerror=1>", capability: "unsupported", moves: [], reason: "nope" })
  const c = new TeleportPickerController({ catalog: async () => [evil], plan: async () => { throw new Error() }, run: async () => { throw new Error() } }, { spaceName: "A&B" })
  c.dispatch({ type: "loaded", entries: [evil, vscode] })
  const html = renderPicker(c, new Map())
  assert.ok(!html.includes("<img onerror"))
  assert.match(html, /&lt;img onerror=1&gt;/)
  assert.match(html, /Teleport an app to A&amp;B/)
  assert.match(html, /data-id="x"[^>]*disabled/)
})

// -- the one drop zone -------------------------------------------------------

import {
  DROP_ZONE_CAPTION,
  DROP_ZONE_SEND_FILE,
  DROP_ZONE_TELEPORT_APP,
  dropZoneStatusLine,
  idleDropZone,
  isInsideZone,
  renderTeleportDropZone,
  sendingLabel,
} from "../dist/teleport/index.js"

test("the drop zone is one caption and two actions", () => {
  assert.equal(DROP_ZONE_CAPTION, "Drop a file or window")
  const html = renderTeleportDropZone({})
  assert.equal((html.match(/<p class="caption">/g) ?? []).length, 1)
  assert.ok(html.includes(DROP_ZONE_CAPTION))
  assert.ok(html.includes(`data-act="send-file">${DROP_ZONE_SEND_FILE}`))
  assert.ok(html.includes(`data-act="teleport-app">${DROP_ZONE_TELEPORT_APP}`))
  assert.ok(!html.includes("data-drop-target"))
  assert.ok(!html.includes('role="status"'))
  assert.ok(renderTeleportDropZone({ over: true }).includes('data-drop-target="true"'))
  assert.ok(!renderTeleportDropZone({ canSendFile: false }).includes("send-file"))
})

test("the drop zone says concretely what it did", () => {
  assert.equal(dropZoneStatusLine(idleDropZone), null)
  assert.equal(sendingLabel(["/tmp/a/report.pdf"]), "report.pdf")
  assert.equal(sendingLabel(["a", "b", "c"]), "3 files")
  assert.equal(dropZoneStatusLine({ kind: "sending", label: "report.pdf" }), "Sending report.pdf…")
  assert.equal(
    dropZoneStatusLine({ kind: "sent", files: [{ name: "report.pdf", bytes: 2048, dest: "~/Downloads" }] }),
    "report.pdf (2.0 KB) verified in ~/Downloads",
  )
  assert.equal(
    dropZoneStatusLine({ kind: "sent", files: [{ name: "a", bytes: 1024, dest: "d" }, { name: "b", bytes: 1024, dest: "d" }] }),
    "2 files (2.0 KB) verified",
  )
  const failed = renderTeleportDropZone({ status: { kind: "failed", message: "<no space>" } })
  assert.ok(failed.includes('data-kind="failed"'))
  assert.ok(failed.includes("&lt;no space&gt;"), "escaped")
  const busy = renderTeleportDropZone({ status: { kind: "sending", label: "x" } })
  assert.ok(busy.includes('data-busy="true"'))
  assert.equal((busy.match(/ disabled/g) ?? []).length, 2)
})

test("the drop zone hit-test", () => {
  const el = { getBoundingClientRect: () => ({ left: 10, top: 20, right: 110, bottom: 70, width: 100, height: 50 }) }
  assert.equal(isInsideZone(el, { x: 10, y: 20 }), true)
  assert.equal(isInsideZone(el, { x: 60, y: 45 }), true)
  assert.equal(isInsideZone(el, { x: 111, y: 45 }), false)
  assert.equal(isInsideZone(null, { x: 0, y: 0 }), false)
  const hidden = { getBoundingClientRect: () => ({ left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 }) }
  assert.equal(isInsideZone(hidden, { x: 0, y: 0 }), false)
})
