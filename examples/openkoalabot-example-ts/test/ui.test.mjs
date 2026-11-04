// The page's DOM pieces under happy-dom: the image dropdown lists exactly
// the published entries of the shared list, the wizard walks its steps and
// posts one plan, and Delete in the Space section needs a confirmation.
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { before, test } from "node:test"
import { Window } from "happy-dom"

const listPath = new URL("../../../libs/images/sandbox-images.json", import.meta.url)
const file = JSON.parse(readFileSync(listPath, "utf8"))
const published = file.images.filter((i) => i.published)

let ui
before(async () => {
  const window = new Window()
  globalThis.window = window
  globalThis.document = window.document
  globalThis.Event = window.Event
  const images = await import("../dist/ui/images.js")
  images.useImageList(file)
  ui = {
    images,
    wizard: await import("../dist/ui/wizard.js"),
    view: await import("../dist/ui/wizardView.js"),
    space: await import("../dist/ui/spaceSection.js"),
    blocks: await import("../dist/ui/blocks.js"),
    activity: await import("../dist/ui/activity.js"),
    computer: await import("../dist/ui/computer.js"),
    errors: await import("../dist/ui/errors.js"),
  }
})

const click = (el) => {
  assert.ok(el, "element exists")
  el.click()
}

test("the image dropdown offers exactly the published entries, in file order", () => {
  const select = ui.view.renderImageSelect(published[0].ref, () => {})
  const values = [...select.querySelectorAll("option")].map((o) => o.value)
  assert.deepEqual(values, published.map((i) => i.ref))
  for (const u of file.images.filter((i) => !i.published)) assert.ok(!values.includes(u.ref), `${u.ref} is not offered`)
  const labels = [...select.querySelectorAll("optgroup")].map((g) => g.getAttribute("label"))
  const want = []
  for (const i of published) {
    const label = file.groups.find((g) => g.id === i.group).label
    if (want.at(-1) !== label) want.push(label)
  }
  assert.deepEqual(labels, want)
  for (const i of published) assert.ok(select.textContent.includes(`${i.name} (${i.ref})`))
})

test("the wizard disables what an image does not support and posts one plan", async () => {
  const plans = []
  const el = ui.view.renderWizard({ cloud: false, onCancel: () => {}, onAddByAddress: () => {}, onCreate: async (plan) => (plans.push(plan), true) })
  document.body.append(el)
  // macOS: no cloud.
  const mac = published.find((i) => i.os === "macos")
  const select = el.querySelector("select")
  select.value = mac.ref
  select.dispatchEvent(new Event("change"))
  const [cloud, local] = el.querySelectorAll(".seg .tile")
  assert.equal(cloud.disabled, true)
  assert.equal(local.disabled, false)
  assert.ok(local.classList.contains("on"))
  for (let i = 0; i < 3; i++) click(el.querySelector('[data-action="continue"]'))
  assert.match(el.textContent, /Lume/)
  click(el.querySelector('[data-action="create"]'))
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(plans.length, 1)
  assert.equal(plans[0].image, mac.ref)
  assert.equal(plans[0].target, "local")
  assert.ok(ui.wizard.isDnsLabel(plans[0].name))
  el.remove()
})

test("the wizard refuses a name that is not a DNS label", () => {
  const s = { ...ui.wizard.initialState(false), step: 2, name: "Not A Label" }
  assert.match(ui.wizard.blocker(s), /lowercase/)
  assert.equal(ui.wizard.next(s).step, 2)
})

test("Space section: picking selects without deleting; Delete asks first", () => {
  const calls = { select: [], delete: [] }
  const spaces = [
    { id: "cloud:desk", name: "desk", provider: "cloud" },
    { id: "direct:lab", name: "lab", provider: "direct" },
  ]
  const el = ui.space.renderSpaceSection({ spaces, selected: "cloud:desk", onSelect: (id) => calls.select.push(id), onNewSpace: () => {}, onDelete: (id) => calls.delete.push(id) })
  document.body.append(el)
  const select = el.querySelector("select")
  select.value = "direct:lab"
  select.dispatchEvent(new Event("change"))
  click(select)
  assert.deepEqual(calls.select, ["direct:lab"])
  assert.deepEqual(calls.delete, [])
  click(el.querySelector('[data-action="delete"]'))
  assert.deepEqual(calls.delete, [], "nothing deleted before confirming")
  assert.match(el.textContent, /Delete desk\?/)
  assert.match(el.textContent, /deletes the Cua Cloud Space/)
  click(el.querySelector('[data-action="cancel"]'))
  assert.equal(el.querySelector('[role="dialog"]'), null)
  assert.deepEqual(calls.delete, [])
  click(el.querySelector('[data-action="delete"]'))
  click(el.querySelector('[data-action="confirm-delete"]'))
  assert.deepEqual(calls.delete, ["cloud:desk"])
  el.remove()
})

test("thread blocks: bubbles, prose, a muted activity group, approval and file cards", () => {
  const items = [
    { turn: 0, kind: "user", text: "tidy", steps: [] },
    { turn: 0, kind: "activity", text: "2 steps", steps: ["Install node: cached", "Tool Terminal completed"] },
    { turn: 0, kind: "message", text: "Looking.\nDelete 3? (y/n)", steps: [] },
  ]
  const b = ui.blocks.toBlocks(items, [{ turn: 0, name: "a.pdf", detail: "1 KB" }])
  assert.deepEqual(b.map((x) => x.kind), ["user", "bot"])
  assert.deepEqual(b[1].items.map((i) => i.kind), ["activity", "text", "approval", "file"])
  assert.deepEqual(b[1].items[0], { kind: "activity", summary: "2 steps", steps: ["Install node: cached", "Tool Terminal completed"] })
  assert.equal(ui.blocks.awaitingApproval(ui.blocks.toBlocks(items)), true)
  assert.equal(ui.blocks.rowPreview("Done.", items), "Done.")
  assert.equal(ui.blocks.rowPreview("", items), "You: tidy", "activity is never the preview")
})

test("an activity group is muted, collapsed to its summary, and expands to one line per step", () => {
  let toggled = null
  const el = ui.activity.renderActivity({ summary: "5 steps", steps: ["a", "b", "c", "d", "e"], open: false, onToggle: (o) => (toggled = o) })
  assert.equal(el.tagName, "DETAILS")
  assert.equal(el.className, "activity")
  assert.equal(el.open, false)
  assert.equal(el.querySelector("summary").textContent, "5 steps")
  assert.equal(el.querySelectorAll(".activity-step").length, 5)
  assert.equal(el.querySelector(".bubble"), null)
  el.open = true
  el.dispatchEvent(new window.Event("toggle"))
  assert.equal(toggled, true)
})

test("the window list shows the Space's app icon, and nothing when it has none", () => {
  const rows = [
    { windowId: "1", app: "Xfce4-terminal", title: "Terminal", icon: "data:image/png;base64,iVBORw0KGgo=" },
    { windowId: "2", app: "XCalc", title: "Calculator" },
  ]
  const el = ui.computer.renderWindowList({ windows: rows, isOpen: () => false, canPip: true, onPip: () => {} })
  const [a, b] = el.querySelectorAll(".win-row")
  const img = a.querySelector("img.win-icon")
  assert.ok(img, "icon shown")
  assert.equal(img.getAttribute("src"), rows[0].icon)
  assert.ok(a.firstElementChild === img, "the icon sits left of the name")
  assert.equal(b.querySelector("img"), null, "no stand-in glyph")
  assert.equal(b.firstElementChild.className, "win-name")
})

test("a rejected token reads as words, with the detail kept", () => {
  const f = ui.errors.friendlyError('status: Unauthenticated, message: "missing or invalid bearer token"')
  assert.equal(f.message, "The Space rejected the token. Check the token and try again.")
  assert.match(f.detail, /bearer token/)
})

test("a Keyvault requires_cua_app refusal reads as Install Cua, with the link", () => {
  const f = ui.errors.friendlyError(
    "Error: teleport refused: requires_cua_app: teleport goes through the Cua Keyvault, which needs the Cua app",
  )
  assert.match(f.message, /^Install Cua to teleport your session\. The Cua app keeps your logins in its Keyvault and asks you before sharing them\./)
  assert.deepEqual(f.link, { label: "Install Cua", url: "https://cua.ai/install" })
  assert.match(f.detail, /requires_cua_app/)
  assert.equal(ui.errors.friendlyError("denied by the user").link, undefined)
})

test("routines and group chats: one-line sidebar rows, the panel, the sheet's bound, the thread", async () => {
  const cw = await import("../dist/ui/coworkers.js")
  const bots = [{ id: "ada", name: "Ada" }, { id: "bo", name: "Bo" }, { id: "cy", name: "Cy" }]
  const routines = [{ id: "r1", botID: "ada", title: "Sweep", prompt: "triage", schedule: { kind: "dailyAt", hour: 8, minute: 0 }, isEnabled: false, label: "Every day at 8:00 AM" }]
  let opened = null
  const side = cw.renderRoutinesSection({ routines, bots, selectedBot: null, onOpen: (id) => (opened = id) })
  const row = side.querySelector(".cw-row")
  assert.equal(row.textContent, "Sweep · Ada")
  assert.ok(row.classList.contains("off"), "a disabled routine is dimmed, nothing more")
  click(row)
  assert.equal(opened, "ada")
  const groups = cw.renderGroupsSection({ groups: [], selected: null, botCount: 1, onOpen: () => {}, onNew: () => {} })
  assert.equal(groups.querySelector(".cw-add").disabled, true, "New group chat needs two Bots")

  const made = []
  const toggles = []
  const panel = cw.renderRoutinesPanel({ bot: bots[0], routines, onCreate: (r) => made.push(r), onToggle: (id, on) => toggles.push([id, on]), onRun: () => {}, onDelete: () => {} })
  click(panel.querySelector('[data-action="create"]'))
  assert.equal(panel.querySelector(".cw-error").textContent, "Name the routine.")
  const [title, prompt] = panel.querySelectorAll(".cw-form input.input")
  title.value = "Standup"
  prompt.value = "post status"
  const kind = panel.querySelector('select[aria-label="Schedule"]')
  kind.value = "everyMinutes"
  kind.dispatchEvent(new window.Event("change"))
  panel.querySelector('input[aria-label="Minutes"]').value = "15"
  click(panel.querySelector('[data-action="create"]'))
  assert.deepEqual(made, [{ title: "Standup", prompt: "post status", schedule: { kind: "everyMinutes", minutes: 15 } }])
  assert.deepEqual(cw.readRoutineForm({ title: "a", prompt: "b", kind: "weeklyOn", minutes: "", time: "13:05", weekday: "2" }), { title: "a", prompt: "b", schedule: { kind: "weeklyOn", weekday: 2, hour: 13, minute: 5 } })

  const created = []
  const sheet = cw.renderNewGroupSheet({ bots, onCancel: () => {}, onCreate: (t, m) => created.push([t, m]) })
  const create = sheet.querySelector('[data-action="create"]')
  const boxes = [...sheet.querySelectorAll('input[type="checkbox"]')]
  assert.equal(create.disabled, true)
  boxes[0].checked = true
  boxes[0].dispatchEvent(new window.Event("change"))
  assert.equal(create.disabled, true, "one Bot is not a group")
  boxes[2].checked = true
  boxes[2].dispatchEvent(new window.Event("change"))
  assert.equal(create.disabled, false)
  click(create)
  assert.deepEqual(created, [["", ["ada", "cy"]]])

  const thread = cw.renderGroupThread({
    group: {
      id: "g",
      title: "Standup",
      memberIDs: ["ada", "bo"],
      membershipLabel: "2 of 6 bots",
      working: ["bo"],
      messages: [
        { id: "1", speaker: { kind: "human" }, text: "Status?", undelivered: false },
        { id: "2", speaker: { kind: "bot", botID: "ada" }, text: "shipped", undelivered: false },
        { id: "3", speaker: { kind: "bot", botID: "bo" }, text: "Did not receive that message: busy", undelivered: true },
      ],
    },
    names: { ada: "Ada", bo: "Bo" },
  })
  const lines = [...thread.querySelectorAll(".cw-line")]
  assert.deepEqual(lines.map((l) => l.querySelector(".author").textContent), ["Ada", "Bo"])
  assert.ok(lines[1].classList.contains("undelivered"))
  assert.equal(thread.querySelector(".cw-typing").textContent, "Bo is working")
})

test("presence cursors: the others are drawn where they are, never mine or a hidden one", async () => {
  const cw = await import("../dist/ui/coworkers.js")
  const layer = cw.renderCursors(
    [
      { participantId: "me", displayName: "Operator", color: "#111", agent: false, cursor: { x: 0.5, y: 0.5, visible: true } },
      { participantId: "k", displayName: "Koala", color: "#e11", agent: true, cursor: { x: 0.25, y: 0.75, visible: true } },
      { participantId: "h", displayName: "Hidden", color: "#1e1", agent: false, cursor: { x: 0.1, y: 0.1, visible: false } },
      { participantId: "n", displayName: "Nowhere", color: "#11e", agent: false },
    ],
    "me",
  )
  const drawn = [...layer.querySelectorAll(".cursor")]
  assert.deepEqual(drawn.map((c) => c.dataset.participant), ["k"])
  assert.match(drawn[0].getAttribute("style"), /left:25\.00%;top:75\.00%/)
  assert.equal(drawn[0].querySelector(".cursor-name").textContent, "Koala")
})

test("a Bot's avatar background is its presence cursor's color", async () => {
  const cw = await import("../dist/ui/coworkers.js")
  const { agentIdentity, presenceColor } = await import("@trycua/cua/spaces/presence")
  cw.useBotAvatarImage("koala.svg")
  const hex = (style, prop) => (style.match(new RegExp(`(?:^|;)\\s*${prop}:\\s*(#[0-9a-fA-F]{6})`)) ?? [])[1]?.toLowerCase()
  for (const botID of ["ada", "bo", "openkoalabots-koala-1"]) {
    // The cursor a Bot draws when it joins presence with the SDK's agent identity.
    const who = agentIdentity(botID, botID)
    const layer = cw.renderCursors([{ participantId: "p", displayName: botID, color: who.color, agent: true, cursor: { x: 0.5, y: 0.5, visible: true } }], "me")
    const cursorColor = layer.querySelector("path").getAttribute("fill").toLowerCase()
    // The avatar in the Bot list and headers, in group attribution and in the picker.
    const own = cw.botAvatar(botID)
    const thread = cw.renderGroupThread({
      group: { id: "g", title: "t", memberIDs: [botID, "x"], membershipLabel: "2 of 6 bots", working: [], messages: [{ id: "1", speaker: { kind: "bot", botID }, text: "hi", undelivered: false }] },
      names: {},
    })
    const sheet = cw.renderNewGroupSheet({ bots: [{ id: botID, name: botID }], onCancel() {}, onCreate() {} })
    for (const a of [own, thread.querySelector(`.avatar[data-bot="${botID}"]`), sheet.querySelector(`.avatar[data-bot="${botID}"]`)]) {
      assert.ok(a, botID)
      assert.equal(hex(a.getAttribute("style"), "background"), cursorColor, botID)
      assert.equal(a.querySelector("img").getAttribute("src"), "koala.svg")
    }
    assert.equal(cursorColor, presenceColor(botID))
    assert.equal(cw.botAvatarColor(botID), cursorColor)
    // Chat lines stay neutral.
    assert.equal(thread.querySelector(".bubble-bot"), null)
  }
})

test("while a Bot is present, its avatar follows the color the server assigned its cursor", async () => {
  const cw = await import("../dist/ui/coworkers.js")
  const { PresenceRoster, presenceColor } = await import("@trycua/cua/spaces/presence")
  const stable = presenceColor("ada")
  // The operator already holds Ada's stable color, so the server gave Ada another.
  const roster = PresenceRoster.from({ participantId: "me", principalId: "operator", displayName: "Operator", color: stable, kind: "human" })
  roster.apply({ kind: "joined", participant: { participantId: "p-ada", principalId: "ada", displayName: "Ada", color: "#123456", kind: "agent" } })
  try {
    assert.equal(cw.setAssignedColors(cw.assignedColorsFrom(roster.entries)), true)
    const cursor = cw.renderCursors(roster.others.map((e) => ({ ...e, cursor: { x: 0.5, y: 0.5, visible: true } })), "me")
    const cursorColor = cursor.querySelector("path").getAttribute("fill").toLowerCase()
    assert.equal(cursorColor, "#123456")
    assert.equal(cw.botAvatarColor("ada"), cursorColor)
    assert.match(cw.botAvatar("ada").getAttribute("style"), /background:#123456/)
    assert.equal(cw.setAssignedColors(cw.assignedColorsFrom(roster.entries)), false, "no change, no re-render")
    // Gone from the roster: back to the stable color.
    roster.apply({ kind: "left", participantId: "p-ada" })
    cw.setAssignedColors(cw.assignedColorsFrom(roster.entries))
    assert.equal(cw.botAvatarColor("ada"), stable)
  } finally {
    cw.setAssignedColors({})
  }
})

test("presence cursors follow the SDK's PresenceView: shaped, faded when idle, gone on run end or a missed heartbeat", async () => {
  const cw = await import("../dist/ui/coworkers.js")
  const { PresenceView } = await import("@trycua/cua/spaces/presence")
  const person = (id, name, kind = "human") => ({ participantId: id, principalId: id, displayName: name, color: "#e6194b", kind })
  const cursor = (x, y, receivedMs, shape = "arrow") => ({ displayId: "", x, y, visible: true, pressed: false, shape, shapeSource: shape === "arrow" ? "unspecified" : "hit_test", atMs: 0, receivedMs })
  const t0 = 1_000_000
  const view = PresenceView.from(person("me", "Operator"), [])
  view.apply({ kind: "joined", participant: person("koala", "Koala", "agent") }, t0)
  view.apply({ kind: "joined", participant: person("ada", "Ada") }, t0)
  view.apply({ kind: "cursor_moved", participantId: "koala", cursor: cursor(0.25, 0.75, t0, "pointer") }, t0)
  view.apply({ kind: "cursor_moved", participantId: "ada", cursor: cursor(0.5, 0.5, t0) }, t0)
  const at = (t) => [...cw.renderDrawables(view.drawables(t)).querySelectorAll(".cursor")]
  let drawn = at(t0 + 200)
  assert.deepEqual(drawn.map((c) => c.dataset.participant), ["koala", "ada"])
  assert.equal(drawn[0].dataset.shape, "pointer")
  assert.match(drawn[0].getAttribute("style"), /left:25\.00%;top:75\.00%;opacity:1\.00/)
  assert.match(drawn[0].querySelector(".cursor-art").innerHTML, /fill="#e6194b"/)
  // Idle for 5.15 s: half faded.
  assert.match(at(t0 + 5_150)[0].getAttribute("style"), /opacity:0\.50/)
  // The Bot's run ends: its cursor goes at once.
  view.apply({ kind: "left", participantId: "koala", reason: "run_ended" }, t0 + 5_200)
  view.apply({ kind: "cursor_moved", participantId: "ada", cursor: cursor(0.6, 0.5, t0 + 5_200) }, t0 + 5_200)
  drawn = at(t0 + 5_400)
  assert.deepEqual(drawn.map((c) => c.dataset.participant), ["ada"])
  // A heartbeat that no longer lists Ada removes her.
  view.apply({ kind: "heartbeat", participantIds: ["me"] }, t0 + 5_500)
  assert.equal(at(t0 + 5_600).length, 0)
})
