// The Computer panel under happy-dom: the window list (one line per window,
// a picture-in-picture button each) and the one drop zone for files, apps and
// windows, from the SDK's `<cua-drop-zone>`. No Space, no browser PiP.
import assert from "node:assert/strict"
import { before, test } from "node:test"
import { Window } from "happy-dom"

let ui
let teleport
let pip
before(async () => {
  const window = new Window()
  for (const k of ["window", "document", "HTMLElement", "CustomEvent", "Event", "customElements"]) globalThis[k] = k === "window" ? window : window[k]
  ui = await import("../dist/ui/computer.js")
  teleport = await import("@trycua/cua/teleport")
  pip = await import("@trycua/cua/spaces/pip")
  teleport.defineTeleportElements(window.customElements)
})

const windows = [
  { windowId: "0x1a00003", app: "Firefox", title: "Mozilla Firefox", width: 1200, height: 800 },
  { windowId: "0x2c00001", app: "Thunar", title: "Downloads" },
]

test("the window list is one line per window, with a PiP button each", () => {
  const opened = []
  const open = pip.windowSource(windows[1])
  const el = ui.renderWindowList({ windows, isOpen: (s) => pip.pipKey(s) === pip.pipKey(open), canPip: true, onPip: (w) => opened.push(w.windowId) })
  const rows = el.querySelectorAll(".win-row")
  assert.equal(rows.length, 2)
  assert.deepEqual([...rows].map((r) => r.querySelector(".win-name").textContent), ["Mozilla Firefox", "Thunar · Downloads"])
  for (const r of rows) assert.equal(r.querySelectorAll("span").length, 1, "one line of text per row")
  const buttons = el.querySelectorAll("button")
  assert.equal(buttons[0].getAttribute("aria-pressed"), "false")
  assert.equal(buttons[1].getAttribute("aria-pressed"), "true")
  assert.equal(buttons[1].title, "Close picture in picture")
  buttons[0].click()
  assert.deepEqual(opened, ["0x1a00003"])
})

test("no windows, no list; no PiP in the browser, disabled buttons", () => {
  assert.equal(ui.renderWindowList({ windows: [], isOpen: () => false, canPip: true, onPip: () => {} }), null)
  const el = ui.renderWindowList({ windows, isOpen: () => false, canPip: false, onPip: () => {} })
  for (const b of el.querySelectorAll("button")) assert.equal(b.disabled, true)
})

test("one drop zone: one caption, Send file… and Teleport an app…", () => {
  const got = []
  const zone = ui.renderDropZone({
    status: teleport.idleDropZone,
    onDrop: (dt) => got.push(["drop", dt]),
    onSendFile: () => got.push(["send"]),
    onTeleportApp: () => got.push(["teleport"]),
  })
  document.body.append(zone)
  assert.equal(zone.tagName, "CUA-DROP-ZONE")
  assert.equal(zone.getAttribute("data-teleport-target"), null, "no window-drag target: window drags ship with Cua Spaces")
  const root = zone.shadowRoot
  assert.equal(root.querySelectorAll(".caption").length, 1)
  assert.equal(root.querySelector(".caption").textContent, "Drop a file or window")
  const labels = [...root.querySelectorAll("button")].map((b) => b.textContent)
  assert.deepEqual(labels, ["Send file…", "Teleport an app…"])
  root.querySelector('[data-act="send-file"]').click()
  root.querySelector('[data-act="teleport-app"]').click()
  const dt = { files: [] }
  zone.dispatchEvent(new CustomEvent("cua-drop", { detail: dt }))
  assert.deepEqual(got, [["send"], ["teleport"], ["drop", dt]])

  zone.status = { kind: "sent", files: [{ name: "notes.txt", bytes: 5, dest: "/home/cua/Downloads/openkoalabots/notes.txt" }] }
  assert.equal(root.querySelector('[role="status"]').textContent, "notes.txt (5 B) verified in /home/cua/Downloads/openkoalabots/notes.txt")
  zone.over = true
  assert.equal(root.querySelector(".zone").getAttribute("data-drop-target"), "true")
  zone.remove()
})
