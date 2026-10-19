/**
 * OpenKoalaBots web UI: a Bots roster, one long-lived thread per Bot, and
 * the Bot's computer beside it. Talks to the local server (`npm run serve`)
 * for Spaces, Bots, files and presence, and streams the Space's
 * desktop itself: a media ticket (from the server, or opened directly over
 * gRPC-Web with the wasm `@trycua/cua/browser` client) attaches the `/media`
 * WebSocket, and WebCodecs decodes H.264 into a canvas.
 */
import { MediaSocketState, avcCodecString, encodeControl, parseBinary } from "../src/core/wire.js"
import { attachMedia, closeMedia, openDesktopMedia, type JsonRpc, type OpenedMedia } from "../src/core/webmedia.js"
import { awaitingApproval, rowPreview, statusLabel, toBlocks, type Block, type FileEvent, type ThreadItem } from "../src/ui/blocks.js"
import { renderActivity } from "../src/ui/activity.js"
import { append, h, icon } from "../src/ui/dom.js"
import { friendlyError } from "../src/ui/errors.js"
import { useImageList } from "../src/ui/images.js"
import { renderDropZone, renderWindowList, type WindowRow } from "../src/ui/computer.js"
import { renderSpaceSection } from "../src/ui/spaceSection.js"
import {
  botAvatar,
  setAssignedColors,
  useBotAvatarImage,
  renderDrawables,
  renderGroupThread,
  renderGroupsSection,
  renderNewGroupSheet,
  renderRoutinesPanel,
  renderRoutinesSection,
  type BotRef,
  type CursorView,
  type GroupView,
  type RoutineView,
} from "../src/ui/coworkers.js"
import { APP_TELEPORT_MESSAGE } from "../src/core/teleport.js"
import {
  classifyBrowserDrop,
  defineTeleportElements,
  readDataTransfer,
  idleDropZone,
  sendingLabel,
  type DropZoneFile,
  type DropZoneStatus,
} from "@trycua/cua/teleport"
import { PictureInPicture, desktopSource, pipKey, pipLabel, pipSize, windowSource } from "@trycua/cua/spaces/pip"
import { PresenceView, type PresenceEvent, type PresenceMember } from "@trycua/cua/spaces/presence"
import { renderWizard } from "../src/ui/wizardView.js"
import type { SpacePlan, WizardState } from "../src/ui/wizard.js"
import imageList from "../../../libs/images/sandbox-images.json"
import koalaMark from "./assets/koala-mark.svg"
import koalaPeek from "./assets/koala-peek.svg"
import koalaSleep from "./assets/koala-sleep.svg"

// The shared image list, imported (one source for the docs and every picker).
useImageList(imageList)

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T
const params = new URLSearchParams(location.hash.slice(1))
const token = params.get("token") ?? ""

async function api<T = unknown>(path: string, init: { method?: string; body?: unknown; raw?: Blob; query?: string } = {}): Promise<T> {
  const res = await fetch(`/api/${path}${init.query ?? ""}`, {
    method: init.method ?? (init.body !== undefined || init.raw ? "POST" : "GET"),
    headers: { authorization: `Bearer ${token}`, ...(init.raw ? {} : { "content-type": "application/json" }) },
    body: init.raw ?? (init.body === undefined ? undefined : JSON.stringify(init.body)),
  })
  const value = (await res.json()) as T & { error?: string }
  if (!res.ok) throw new Error(value.error ?? `${res.status}`)
  return value
}

// ------------------------------------------------------------------ state

interface BotView {
  runId: string
  bot: { id: string; name: string; agent: string }
  state: string
  reason: string
  acceptsMessage: boolean
  /** The thread as the SDK's event fold made it. */
  items: ThreadItem[]
  /** The Bot's last message on one line. */
  preview: string
}
interface SpaceView {
  id: string
  name: string
  provider: string
  spacesdVersion?: string
  features: string[]
}
interface Snapshot {
  space: SpaceView | null
  bots: BotView[]
  knownBots?: BotRef[]
  routines?: RoutineView[]
  groups?: GroupView[]
  presence?: { me: string | null; cursors: CursorView[]; colors?: Record<string, string> } | null
}

const AGENTS = [
  { id: "claude-code", label: "Claude Code" },
  { id: "openai-codex", label: "Codex" },
]
const SETTLED = new Set(["idle", "finished", "failed", "crashed", "stopped", "awaiting_input"])

const state = {
  connected: false,
  cloud: false,
  spaces: [] as SpaceView[],
  selectedSpace: null as string | null,
  snap: { space: null, bots: [] } as Snapshot,
  /** A Bot hired in the page that has not had its first task yet. */
  pending: null as { name: string; agent: string } | null,
  selected: null as string | null, // runId, or "pending"
  /** The main pane: a Bot's thread, a Bot's routines, or a group chat. */
  view: { kind: "bot" } as { kind: "bot" } | { kind: "routines"; botID: string } | { kind: "group"; id: string },
  files: {} as Record<string, FileEvent[]>,
  query: "",
  computerOpen: true,
  error: "",
  notice: "",
  stream: { on: true, frames: 0, width: 0, height: 0, status: "" },
}

/** Activity groups the user expanded (`runId:block:item`), kept across renders. */
const openActivity = new Set<string>()

function fail(e: unknown): void {
  state.error = e instanceof Error ? e.message : String(e)
  renderMain()
}

// ------------------------------------------------------------- the frame

const app = $("app")
const sidebar = h("aside", { class: "sidebar" })
const main = h("main", { class: "thread" })
const computer = h("aside", { class: "computer" })
app.append(sidebar, main, computer)

useBotAvatarImage(koalaMark)

function avatar(size = ""): HTMLElement {
  return h("span", { class: `avatar ${size}` }, h("img", { src: koalaMark, alt: "" }))
}

// --------------------------------------------------------------- sidebar

function knownBots(): BotRef[] {
  return state.snap.knownBots ?? []
}

function openView(v: typeof state.view): void {
  state.view = v
  if (v.kind !== "bot") state.selected = null
  render()
}

function renderSidebar(): void {
  const roster = h("nav", { class: "roster" })
  renderRosterInto(roster)
  const search = h("input", {
    placeholder: "Search Bots",
    value: state.query,
    oninput: (e) => {
      state.query = (e.target as HTMLInputElement).value
      roster.replaceChildren()
      renderRosterInto(roster)
    },
  })
  sidebar.replaceChildren(
    h("div", { class: "brand" }, h("img", { src: koalaMark, alt: "" }), h("b", {}, "OpenKoalaBots")),
    h(
      "div",
      { class: "sidebar-top" },
      h("button", { class: "btn new-bot", onclick: () => (state.selectedSpace ? openNewBot() : openWizard()) }, icon("plus"), "New Bot"),
      h("label", { class: "search" }, icon("search"), search),
    ),
    h("div", { class: "section-label" }, "Bots"),
    roster,
    ...(state.snap.space
      ? [
          renderRoutinesSection({
          routines: state.snap.routines ?? [],
          bots: knownBots(),
          selectedBot: state.view.kind === "routines" ? state.view.botID : null,
          onOpen: (botID) => openView({ kind: "routines", botID }),
          }),
          renderGroupsSection({
            groups: state.snap.groups ?? [],
            selected: state.view.kind === "group" ? state.view.id : null,
            botCount: knownBots().length,
            onOpen: (id) => openView({ kind: "group", id }),
            onNew: () => openNewGroup(),
          }),
        ]
      : []),
    renderSpaceSection({
      spaces: state.spaces,
      selected: state.selectedSpace,
      onSelect: (id) => selectSpace(id),
      onNewSpace: () => openWizard(),
      onDelete: (id) =>
        api("spaces/delete", { body: { id } })
          .then(() => refreshSpaces())
          .catch(fail),
    }),
    h(
      "div",
      { class: "account" },
      h("span", { class: "avatar sm initials", style: "background:#5d5d63" }, "OP"),
      h("span", { class: "who" }, h("div", { class: "name" }, "Operator"), h("div", { class: "sub" }, state.connected ? (state.cloud ? "Cua Cloud connected" : "This machine only") : "Connecting…")),
      h("span", { class: `dot ${state.connected ? (state.cloud ? "ready" : "none") : "unknown"}` }),
    ),
  )
}

function renderRosterInto(el: HTMLElement): void {
  const q = state.query.toLowerCase()
  let shown = 0
  if (state.pending && state.pending.name.toLowerCase().includes(q)) {
    el.append(botRow("pending", state.pending.name, "No messages yet", "none", false))
    shown++
  }
  for (const b of state.snap.bots) {
    if (!b.bot.name.toLowerCase().includes(q)) continue
    el.append(botRow(b.runId, b.bot.name, rowPreview(b.preview, b.items) || "No messages yet", b.state, awaitingApproval(toBlocks(b.items)), b.bot.id))
    shown++
  }
  const total = state.snap.bots.length + (state.pending ? 1 : 0)
  if (total === 0) el.append(h("div", { class: "roster-empty" }, "No Bots yet. Hire one to get started."))
  else if (shown === 0) el.append(h("div", { class: "roster-empty" }, `No Bot matches “${state.query}”.`))
}

function botRow(id: string, name: string, last: string, status: string, needsYou: boolean, botID?: string): HTMLElement {
  return h(
    "button",
    {
      class: `bot-row ${state.selected === id ? "on" : ""}`,
      onclick: () => {
        state.selected = id
        state.view = { kind: "bot" }
        render()
      },
    },
    botID ? botAvatar(botID) : avatar(),
    h("span", { class: "who" }, h("span", { class: "name" }, name), h("span", { class: "last" }, last)),
    h("span", { class: "meta" }, needsYou ? h("span", { class: "badge" }, "1") : h("span", { class: `dot ${status}`, title: statusLabel(status) })),
  )
}

// ------------------------------------------------------------------ main

const composers = new Map<string, HTMLElement>()

/** One composer per Bot (kept across renders, so a draft survives). */
function composer(key: string, opts: { big?: boolean; bare?: boolean; placeholder: string; agent: string; agentLocked?: boolean; onAgent?: (a: string) => void; onSend: (t: string) => void; onAttach?: (f: File) => void; lead?: HTMLElement }): HTMLElement {
  const k = `${key}:${opts.big ? "big" : "dock"}`
  const cached = composers.get(k)
  if (cached) return cached
  const area = h("textarea", { rows: opts.big ? 2 : 1, placeholder: opts.placeholder })
  const send = h("button", { class: "send", title: "Send", disabled: true }, icon("arrowUp", 18))
  const submit = () => {
    const t = area.value.trim()
    if (!t || send.disabled) return
    area.value = ""
    send.disabled = true
    opts.onSend(t)
  }
  area.addEventListener("input", () => (send.disabled = !area.value.trim()))
  area.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault()
      submit()
    }
  })
  send.addEventListener("click", submit)
  const file = h("input", {
    type: "file",
    hidden: true,
    onchange: (e) => {
      const f = (e.target as HTMLInputElement).files?.[0]
      if (f) opts.onAttach?.(f)
      ;(e.target as HTMLInputElement).value = ""
    },
  })
  const picker = h(
    "select",
    { class: "agent-picker", title: opts.agentLocked ? "The thread keeps its agent" : "Agent", disabled: !!opts.agentLocked, onchange: (e) => opts.onAgent?.((e.target as HTMLSelectElement).value) },
    ...AGENTS.map((a) => h("option", { value: a.id }, a.label)),
  )
  picker.value = opts.agent
  const el = h(
    "div",
    { class: `composer ${opts.big ? "big" : ""}` },
    opts.lead ?? null,
    area,
    h(
      "div",
      { class: "tools" },
      opts.bare ? null : h("button", { class: "icon-btn", title: "Attach a file", disabled: !opts.onAttach, onclick: () => file.click() }, icon("paperclip")),
      opts.bare ? null : file,
      opts.bare ? null : picker,
      h("span", { class: "spacer" }),
      send,
    ),
  )
  composers.set(k, el)
  return el
}

function empty(art: string, title: string, text: string, ...rest: Array<HTMLElement | null>): HTMLElement {
  return h("div", { class: "empty" }, h("img", { class: "art", src: art, alt: "" }), h("h1", {}, title), h("p", {}, text), ...rest)
}

function banner(kind: "error" | "", text: string, clear: () => void): HTMLElement {
  const f = kind === "error" ? friendlyError(text) : { message: text, detail: "" }
  return h(
    "div",
    { class: `banner ${kind}` },
    icon(kind === "error" ? "alert" : "check"),
    h(
      "span",
      { class: "selectable" },
      f.message,
      f.link ? h("a", { class: "btn btn-primary", href: f.link.url, target: "_blank", rel: "noopener noreferrer" }, f.link.label) : null,
      f.detail ? h("details", {}, h("summary", {}, "Details"), f.detail) : null,
    ),
    h(
      "button",
      {
        class: "icon-btn",
        title: "Dismiss",
        onclick: () => {
          clear()
          renderMain()
        },
      },
      icon("close"),
    ),
  )
}

let lastScrollKey = ""

function renderMain(): void {
  if (state.view.kind !== "bot" && state.snap.space) return renderSideView(state.view)
  const bot = state.snap.bots.find((b) => b.runId === state.selected) ?? null
  const pending = state.selected === "pending" ? state.pending : null
  const header = h("header", { class: "pane-header" })
  if (bot || pending) {
    const status = bot?.state ?? ""
    append(header, [
      bot ? botAvatar(bot.bot.id, "sm") : avatar("sm"),
      h("span", { class: "title" }, bot?.bot.name ?? pending!.name),
      h("span", { class: `dot ${status || "none"}` }),
      h("span", { class: "subtitle" }, `${statusLabel(status)} · ${AGENTS.find((a) => a.id === (bot?.bot.agent ?? pending!.agent))?.label ?? ""}`),
      bot ? h("button", { class: "icon-btn", title: "Routines", onclick: () => openView({ kind: "routines", botID: bot.bot.id }) }, icon("clock")) : null,
    ])
  } else header.append(h("span", { class: "title" }, state.snap.space?.name ?? "OpenKoalaBots"))
  header.append(
    h("span", { class: "spacer" }),
    h(
      "button",
      {
        class: `icon-btn ${state.computerOpen ? "on" : ""}`,
        title: state.computerOpen ? "Hide the Computer" : "Show the Computer",
        onclick: () => {
          state.computerOpen = !state.computerOpen
          app.classList.toggle("computer-closed", !state.computerOpen)
          renderMain()
        },
      },
      icon("panelRight"),
    ),
  )
  const parts: HTMLElement[] = [header]
  if (state.error) parts.push(banner("error", state.error, () => (state.error = "")))
  if (state.notice) parts.push(banner("", state.notice, () => (state.notice = "")))

  if (!state.snap.space) {
    parts.push(
      empty(
        koalaPeek,
        "Give your Bots a computer",
        "Bots work in a Space: a Linux, Windows or macOS desktop in the cloud or on this machine.",
        h(
          "div",
          { class: "suggestions" },
          h("button", { class: "btn btn-primary", onclick: () => openWizard() }, icon("plus"), "New Space"),
          h("button", { class: "btn", onclick: () => openAddress() }, icon("link"), "Add by address"),
        ),
      ),
    )
  } else if (pending) {
    parts.push(
      empty(
        koalaSleep,
        `What should ${pending.name} work on?`,
        "Give it a task. It keeps this thread, so follow-ups pick up where it left off.",
        composer(`pending:${pending.name}`, { big: true, placeholder: `Message ${pending.name}…`, agent: pending.agent, onAgent: (a) => (pending.agent = a), onSend: (t) => hire(pending.name, pending.agent, t) }),
      ),
    )
  } else if (!bot) {
    const name = h("input", { value: "Koala", "aria-label": "Bot name", style: "border:0;background:transparent;outline:none;width:120px;font-weight:600;color:var(--text)" })
    let agent = AGENTS[0].id
    parts.push(
      empty(
        koalaPeek,
        "Hire a Bot to get started",
        "Name it and give it a first task. It works on its own computer, and you can watch or step in.",
        composer("hire", {
          big: true,
          placeholder: "Describe the first task…",
          agent,
          onAgent: (a) => (agent = a),
          onSend: (t) => hire((name as HTMLInputElement).value.trim() || "Koala", agent, t),
          lead: h("label", { class: "attach-chip", style: "margin-left:-6px" }, "Name", name),
        }),
      ),
    )
  } else {
    const blocks = toBlocks(bot.items, state.files[bot.runId] ?? [])
    const col = h("div", { class: "col" }, ...blocks.map((b, i) => blockView(b, bot, i === blocks.length - 1, i)))
    if (!SETTLED.has(bot.state))
      col.append(h("div", { class: "msg msg-bot" }, botAvatar(bot.bot.id), h("div", { class: "body" }, h("div", { class: "card status" }, h("span", { class: "spinner" }), h("b", {}, bot.bot.name), " is working in its Space…"))))
    const scroll = h("div", { class: "scroll" }, col)
    parts.push(
      scroll,
      h(
        "div",
        { class: "dock" },
        bot.acceptsMessage ? null : h("div", { class: "note" }, icon("info"), `${bot.bot.name} is mid-turn: a message now would be refused, not queued.`),
        composer(bot.runId, {
          placeholder: `Message ${bot.bot.name}…`,
          agent: bot.bot.agent,
          agentLocked: true,
          onSend: (t) => message(bot.runId, t),
          onAttach: (f) => attachFile(f, bot.runId),
        }),
      ),
    )
    const key = `${bot.runId}:${JSON.stringify(bot.items.at(-1) ?? null)}:${bot.items.length}:${bot.state}`
    if (key !== lastScrollKey) {
      lastScrollKey = key
      requestAnimationFrame(() => (scroll.scrollTop = scroll.scrollHeight))
    }
  }
  const prev = main.querySelector(".scroll")
  const keep = prev ? prev.scrollTop : 0
  main.replaceChildren(...parts)
  const now = main.querySelector(".scroll")
  if (now && prev) now.scrollTop = keep
}

/** The Routines panel of a Bot, or a group chat, in the main pane. */
function renderSideView(v: { kind: "routines"; botID: string } | { kind: "group"; id: string }): void {
  const header = h("header", { class: "pane-header" })
  const parts: HTMLElement[] = [header]
  const names = Object.fromEntries(knownBots().map((b) => [b.id, b.name]))
  if (v.kind === "routines") {
    const bot = knownBots().find((b) => b.id === v.botID) ?? { id: v.botID, name: v.botID }
    const thread = state.snap.bots.filter((b) => b.bot.id === bot.id).at(-1)
    header.append(
      icon("clock"),
      h("span", { class: "title" }, `Routines · ${bot.name}`),
      h("span", { class: "spacer" }),
    )
    if (thread) header.append(h("button", { class: "btn btn-ghost", onclick: () => ((state.selected = thread.runId), openView({ kind: "bot" })) }, "Open thread"))
    if (state.error) parts.push(banner("error", state.error, () => (state.error = "")))
    parts.push(
      h(
        "div",
        { class: "scroll" },
        h(
          "div",
          { class: "col" },
          renderRoutinesPanel({
            bot,
            routines: (state.snap.routines ?? []).filter((r) => r.botID === bot.id),
            onCreate: (r) => void api("routines", { body: { botID: bot.id, ...r } }).then(refreshState, fail),
            onToggle: (id, enabled) => void api(`routines/${encodeURIComponent(id)}`, { body: { enabled } }).then(refreshState, fail),
            onRun: (id) => void api(`routines/${encodeURIComponent(id)}/run`, { body: {} }).then(refreshState, fail),
            onDelete: (id) => void api(`routines/${encodeURIComponent(id)}/delete`, { body: {} }).then(refreshState, fail),
          }),
        ),
      ),
    )
  } else {
    const g = (state.snap.groups ?? []).find((x) => x.id === v.id)
    if (!g) {
      state.view = { kind: "bot" }
      return renderMain()
    }
    header.append(h("span", { class: "title" }, g.title), h("span", { class: "subtitle" }, g.membershipLabel))
    if (state.error) parts.push(banner("error", state.error, () => (state.error = "")))
    const scroll = renderGroupThread({ group: g, names })
    parts.push(
      scroll,
      h(
        "div",
        { class: "dock" },
        composer(`group:${g.id}`, {
          bare: true,
          placeholder: `Message ${g.memberIDs.map((id) => names[id] ?? id).join(", ")}…`,
          agent: "",
          onSend: (text) => void api(`groups/${encodeURIComponent(g.id)}/send`, { body: { text } }).then(refreshState, fail),
        }),
      ),
    )
    const key = `${g.id}:${g.messages.length}:${g.working.length}`
    if (key !== lastScrollKey) {
      lastScrollKey = key
      requestAnimationFrame(() => (scroll.scrollTop = scroll.scrollHeight))
    }
  }
  main.replaceChildren(...parts)
}

async function refreshState(): Promise<void> {
  state.snap = await api<Snapshot>("state")
  render()
}

function openNewGroup(): void {
  const el = renderNewGroupSheet({
    bots: knownBots(),
    onCancel: () => close(),
    onCreate: (title, members) => {
      api<GroupView>("groups", { body: { title, members } })
        .then(async (g) => {
          close()
          await refreshState()
          openView({ kind: "group", id: g.id })
        })
        .catch((e) => {
          close()
          fail(e)
        })
    },
  })
  const close = sheet(el)
}

function blockView(block: Block, bot: BotView, live: boolean, index: number): HTMLElement {
  if (block.kind === "user") return h("div", { class: "msg msg-user" }, h("div", { class: "bubble" }, block.text))
  const items = block.items.map((it, i) => {
    switch (it.kind) {
      case "text":
        return h("div", { class: "text" }, it.text)
      case "activity": {
        const key = `${bot.runId}:${index}:${i}`
        return renderActivity({
          summary: it.summary,
          steps: it.steps,
          open: openActivity.has(key),
          onToggle: (open) => (open ? openActivity.add(key) : openActivity.delete(key)),
        })
      }
      case "approval":
        return h(
          "div",
          { class: "card approval" },
          h("div", { class: "card-head" }, icon("alert"), `${bot.bot.name} needs your approval`),
          h("div", { class: "card-body" }, it.prompt),
          live && i === block.items.length - 1
            ? h(
                "div",
                { class: "actions" },
                h("button", { class: "btn btn-primary", onclick: () => message(bot.runId, it.options[0]) }, "Approve"),
                h("button", { class: "btn", onclick: () => message(bot.runId, it.options[1]) }, "Deny"),
              )
            : null,
        )
      case "file":
        return h("div", { class: "card file" }, h("span", { class: "file-icon" }, icon("file")), h("span", {}, h("div", { class: "name" }, it.name), h("div", { class: "sub" }, it.detail)))
    }
  })
  return h("div", { class: "msg msg-bot" }, botAvatar(bot.bot.id), h("div", { class: "body" }, h("div", { class: "author" }, bot.bot.name), ...items))
}

// ------------------------------------------------------------------ actions

async function hire(name: string, agent: string, prompt: string): Promise<void> {
  try {
    const r = await api<{ runId: string }>("bots", { body: { name, agent, prompt } })
    state.pending = null
    state.selected = r.runId
    state.error = ""
    render()
  } catch (e) {
    fail(e)
  }
}

async function message(runId: string, text: string): Promise<void> {
  try {
    const r = await api<{ accepted: boolean; reason: string }>(`bots/${encodeURIComponent(runId)}/message`, { body: { text } })
    if (!r.accepted) state.error = `Refused: ${r.reason}`
    renderMain()
  } catch (e) {
    fail(e)
  }
}

async function attachFile(file: File, runId: string | null, quiet = false): Promise<{ bytes: number; dest: string } | null> {
  try {
    const r = await api<{ files: Array<{ path: string; size: number }>; bytes: number; verified: boolean }>("files", { raw: file, query: `?name=${encodeURIComponent(file.name)}` })
    const dest = r.files[0]?.path ?? "~/Downloads/openkoalabots"
    const detail = `${formatBytes(Number(r.bytes))} to ${dest}${r.verified ? ", SHA-256 verified" : ""}`
    if (runId) {
      const bot = state.snap.bots.find((b) => b.runId === runId)
      const turn = bot?.items.at(-1)?.turn ?? 0
      ;(state.files[runId] ??= []).push({ turn, name: file.name, detail })
    } else if (!quiet) state.notice = `Sent ${file.name}: ${detail}`
    renderMain()
    return r.verified ? { bytes: Number(r.bytes), dest } : null
  } catch (e) {
    fail(e)
    return null
  }
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}

async function selectSpace(id: string): Promise<void> {
  try {
    await api("spaces/select", { body: { id } })
    state.selectedSpace = id
    state.selected = null
    state.pending = null
    await refreshSpaces()
  } catch (e) {
    fail(e)
  }
}

async function refreshSpaces(): Promise<void> {
  const r = await api<{ spaces: SpaceView[]; selected: string | null }>("spaces")
  state.spaces = r.spaces
  state.selectedSpace = r.selected
  render()
}

// ----------------------------------------------------------------- sheets

function sheet(el: HTMLElement): () => void {
  document.body.append(el)
  return () => el.remove()
}

function openWizard(initial?: Partial<WizardState>): void {
  let close = () => {}
  const el = renderWizard({
    cloud: state.cloud,
    initial,
    onCancel: () => close(),
    onAddByAddress: () => {
      close()
      openAddress()
    },
    onCreate: async (plan: SpacePlan, openWhenReady: boolean) => {
      try {
        const info = await api<SpaceView>("spaces/create", { body: { plan } })
        state.notice = `${info.name} is ready.`
        if (openWhenReady) {
          state.computerOpen = true
          app.classList.remove("computer-closed")
        }
        await refreshSpaces()
        return true
      } catch (e) {
        fail(e)
        return false
      }
    },
  })
  close = sheet(el)
}

function field(label: string, input: HTMLElement): HTMLElement {
  return h("label", { class: "field" }, h("span", {}, label), input)
}

function openAddress(): void {
  const url = h("input", { class: "input", placeholder: "10.0.0.5:3211", autofocus: true })
  const tok = h("input", { class: "input", type: "password", placeholder: "The spacesd token" })
  const name = h("input", { class: "input", placeholder: "Optional" })
  const add = h("button", { class: "btn btn-primary" }, "Add Space")
  const el = h(
    "div",
    { class: "scrim", role: "dialog", "aria-modal": "true" },
    h(
      "div",
      { class: "sheet sm" },
      h("div", { class: "sheet-head" }, h("h2", {}, "Add a Space by address"), h("p", {}, "A machine that already runs cua-spacesd. Nothing is created.")),
      h("div", { class: "sheet-body" }, field("Address", url), field("Token", tok), field("Name", name)),
      h("div", { class: "sheet-foot" }, h("span", { class: "spacer" }), h("button", { class: "btn", onclick: () => close() }, "Cancel"), add),
    ),
  )
  add.onclick = async () => {
    add.disabled = true
    try {
      await api("spaces/add", { body: { url: (url as HTMLInputElement).value.trim(), token: (tok as HTMLInputElement).value, name: (name as HTMLInputElement).value.trim() } })
      close()
      await refreshSpaces()
    } catch (e) {
      close()
      fail(e)
    }
  }
  const close = sheet(el)
}

function openNewBot(): void {
  const name = h("input", { class: "input", placeholder: "Research, Inbox, Koala…", autofocus: true })
  const agent = h("select", { class: "select" }, ...AGENTS.map((a) => h("option", { value: a.id }, a.label)))
  const go = () => {
    const n = (name as HTMLInputElement).value.trim()
    if (!n) return
    state.pending = { name: n, agent: (agent as HTMLSelectElement).value }
    state.selected = "pending"
    close()
    render()
  }
  name.addEventListener("keydown", (e) => (e as KeyboardEvent).key === "Enter" && go())
  const el = h(
    "div",
    { class: "scrim", role: "dialog", "aria-modal": "true" },
    h(
      "div",
      { class: "sheet sm" },
      h("div", { class: "sheet-head" }, h("h2", {}, "Hire a Bot"), h("p", {}, "Give it a name. It gets one long-lived thread in the current Space.")),
      h("div", { class: "sheet-body" }, field("Name", name), field("Agent", agent)),
      h("div", { class: "sheet-foot" }, h("span", { class: "spacer" }), h("button", { class: "btn", onclick: () => close() }, "Cancel"), h("button", { class: "btn btn-primary", onclick: go }, "Hire")),
    ),
  )
  const close = sheet(el)
  name.focus()
}

// ------------------------------------------------------- teleport an app

// The SDK's drop zone (`<cua-drop-zone>`) is a plain web component.
defineTeleportElements()

/** "Teleport an app…" ships with Cua Spaces: say so. */
function openTeleportApp(): void {
  state.notice = APP_TELEPORT_MESSAGE
  renderMain()
}

/** An app dragged from Finder or the Dock onto the Space: say app teleport
 * ships with Cua Spaces instead of uploading the bundle. */
function dropApp(dt: DataTransfer): boolean {
  if (classifyBrowserDrop(readDataTransfer(dt), []).kind !== "app") return false
  openTeleportApp()
  return true
}

// --------------------------------------------------------------- computer

const canvas = h("canvas", { width: 1280, height: 800 }) as HTMLCanvasElement
let current: { ws: { close(code?: number): void; send?(d: string): void }; close: () => Promise<void> } | null = null
let streamedSpace: string | null = null
let windowList: WindowRow[] = []
let zoneStatus: DropZoneStatus = idleDropZone
const desktopPainter = painter(canvas, (frames, w, hgt, first) => {
  state.stream.frames = frames
  state.stream.width = w
  state.stream.height = hgt
  if (first || performance.now() - statsTick > 1000) {
    statsTick = performance.now()
    renderComputer()
  }
})

// Picture in picture (`@trycua/cua/spaces/pip`): Document PiP when the
// browser has it, video PiP otherwise; one at a time, of the desktop or of
// one window. A window PiP has its own stream, closed with the PiP.
const pip = new PictureInPicture()
let windowStream: { key: string; close: () => void } | null = null
pip.subscribe((src) => {
  if (windowStream && (!src || pipKey(src) !== windowStream.key)) {
    windowStream.close()
    windowStream = null
  }
  renderComputer()
})

function toggleDesktopPip(): void {
  pip.toggle(desktopSource, canvas, { title: state.snap.space?.name ?? "Desktop" }).catch(fail)
}

function toggleWindowPip(w: WindowRow): void {
  const src = windowSource(w)
  if (pip.isOpen(src)) return pip.close()
  const size = w.width && w.height ? pipSize(w.width, w.height, 1280) : { width: 1280, height: 800 }
  const c = h("canvas", { width: size.width, height: size.height }) as HTMLCanvasElement
  // A first (black) frame, so the video fallback has something to show.
  c.getContext("2d")?.fillRect(0, 0, c.width, c.height)
  // Open first (the PiP needs the click), then attach the window's stream.
  pip
    .open(src, c, { title: pipLabel(src) })
    .then(async () => {
      const t = await api<OpenedMedia & { needsHeaders: boolean }>("stream", { body: { maxFps: 30, maxDimension: 1280, windowId: w.windowId } })
      const paint = painter(c, () => {})
      const { ws } = await attachMedia(WebSocket as never, t, 20_000, paint.feed)
      paint.socket = ws as never
      const close = () => {
        ws.close(1000)
        paint.reset()
        void api("stream/close", { body: { mediaSessionId: t.mediaSessionId } }).catch(() => {})
      }
      if (pip.isOpen(src)) windowStream = { key: pipKey(src), close }
      else close()
    })
    .catch((e) => {
      if (pip.isOpen(src)) pip.close()
      fail(e)
    })
}

async function refreshWindows(): Promise<void> {
  const space = state.snap.space
  if (!space || !space.features.includes("window_stream")) {
    windowList = []
    return
  }
  const r = await api<{ windows: WindowRow[] }>("windows").catch(() => ({ windows: [] as WindowRow[] }))
  const before = JSON.stringify(windowList)
  windowList = r.windows
  if (JSON.stringify(windowList) !== before) renderComputer()
}
setInterval(() => {
  if (state.computerOpen && state.snap.space) void refreshWindows()
}, 5000)

// The one drop zone: files upload (SHA-256 verified); an app bundle or
// "Teleport an app…" shows that app teleport ships with Cua Spaces.
const fileInput = h("input", {
  type: "file",
  multiple: true,
  hidden: true,
  onchange: (e: Event) => {
    const input = e.target as HTMLInputElement
    const files = Array.from(input.files ?? [])
    input.value = ""
    void sendFiles(files)
  },
}) as HTMLInputElement
document.body.append(fileInput)

async function sendFiles(files: File[]): Promise<void> {
  if (!files.length) return
  const run = state.snap.bots.some((b) => b.runId === state.selected) ? state.selected : null
  zoneStatus = { kind: "sending", label: sendingLabel(files.map((f) => f.name)) }
  renderComputer()
  const sent: DropZoneFile[] = []
  for (const f of files) {
    const r = await attachFile(f, run, true)
    if (!r) {
      zoneStatus = { kind: "failed", message: `${f.name} was not sent` }
      return renderComputer()
    }
    sent.push({ name: f.name, bytes: r.bytes, dest: r.dest })
  }
  zoneStatus = { kind: "sent", files: sent }
  renderComputer()
}

function renderComputer(): void {
  const space = state.snap.space
  const streams = !!space && space.features.includes("desktop_stream")
  const live = current && state.stream.frames > 0
  const overlayText = !space ? "No Space yet" : !streams ? "This Space does not stream its desktop" : !state.stream.on ? "Stream stopped" : state.stream.status || "Connecting to the desktop…"
  const screen = h(
    "div",
    { class: "screen" },
    canvas,
    cursorHost,
    live
      ? h("span", { class: "live" }, h("span", { class: "dot" }), `Live · ${state.stream.width}×${state.stream.height}`)
      : h("div", { class: "overlay" }, h("img", { src: koalaSleep, alt: "" }), h("div", {}, overlayText)),
  )
  const body: Array<HTMLElement | null> = [
    screen,
    h("div", { class: "screen-bar" }, h("span", {}, space ? space.name : "No Space"), live ? h("span", {}, `· ${state.stream.frames} frames`) : null, h("span", { class: "spacer" }), presenceEl),
  ]
  if (space) {
    const directUrl = h("input", { class: "input", placeholder: "http://127.0.0.1:3211", id: "direct-url" })
    const directToken = h("input", { class: "input", type: "password", placeholder: "spacesd token", id: "direct-token" })
    body.push(
      renderWindowList({ windows: windowList, isOpen: (src) => pip.isOpen(src), canPip: pip.mode !== "none", onPip: toggleWindowPip }),
      renderDropZone({
        status: zoneStatus,
        onSendFile: () => fileInput.click(),
        onTeleportApp: () => openTeleportApp(),
        onDrop: (dt) => {
          const files = Array.from(dt.files ?? [])
          if (!dropApp(dt)) void sendFiles(files)
        },
      }),
      h(
        "div",
        { class: "panel-card" },
        h("h3", {}, "Space"),
        h(
          "dl",
          { class: "kv" },
          h("dt", {}, "Name"),
          h("dd", {}, space.name),
          h("dt", {}, "Id"),
          h("dd", { class: "selectable", title: space.id }, space.id),
          h("dt", {}, "Provider"),
          h("dd", {}, space.provider),
        ),
        h(
          "details",
          {},
          h("summary", { class: "hint" }, "Connect the stream directly (wasm gRPC-Web)"),
          h(
            "div",
            { style: "display:grid;gap:6px;margin-top:8px" },
            directUrl,
            directToken,
            h("button", { class: "btn", onclick: () => void startDirect((directUrl as HTMLInputElement).value.trim(), (directToken as HTMLInputElement).value) }, "Connect"),
          ),
        ),
      ),
    )
  }
  const desktopPip = pip.isOpen(desktopSource)
  computer.replaceChildren(
    h(
      "header",
      { class: "pane-header" },
      icon("monitor"),
      h("span", { class: "title" }, "Computer"),
      h("span", { class: "spacer" }),
      h(
        "button",
        {
          class: "icon-btn",
          title: state.stream.on ? "Stop the stream" : "Start the stream",
          disabled: !streams,
          onclick: () => {
            state.stream.on = !state.stream.on
            if (!state.stream.on) void stopStream()
            else streamedSpace = null
            syncStream()
          },
        },
        icon(state.stream.on ? "stop" : "play"),
      ),
      h(
        "button",
        {
          class: `icon-btn ${desktopPip ? "on" : ""}`,
          title: desktopPip ? "Close picture in picture" : "Picture in picture",
          disabled: !live || pip.mode === "none",
          onclick: toggleDesktopPip,
        },
        icon("pip"),
      ),
      h(
        "button",
        {
          class: "icon-btn",
          title: "Hide",
          onclick: () => {
            state.computerOpen = false
            app.classList.add("computer-closed")
            renderMain()
          },
        },
        icon("close"),
      ),
    ),
    h("div", { class: "computer-body" }, ...body.filter((n): n is HTMLElement => n !== null)),
  )
}

/** H.264 over the media socket into `target` with WebCodecs, resyncing on
 *  the next keyframe after a decoder error. */
function painter(target: HTMLCanvasElement, onFrame: (frames: number, width: number, height: number, first: boolean) => void) {
  const ctx = target.getContext("2d")
  let decoder: VideoDecoder | null = null
  let frames = 0
  const make = (codec: string) => {
    const d = new VideoDecoder({
      output: (frame) => {
        if (target.width !== frame.displayWidth || target.height !== frame.displayHeight) {
          target.width = frame.displayWidth
          target.height = frame.displayHeight
        }
        ctx?.drawImage(frame, 0, 0)
        frame.close()
      },
      error: () => {
        decoder = null
        self.socket?.send?.(encodeControl("request_keyframe"))
      },
    })
    d.configure({ codec, optimizeForLatency: true })
    return d
  }
  const self = {
    /** The media socket, once attached (for keyframe requests). */
    socket: null as { send?(d: string): void } | null,
    feed(s: MediaSocketState, data: ArrayBuffer): void {
      const p = parseBinary(data)
      if (p.kind !== "video" || p.descriptor.codec !== "h264") return
      if (!decoder) {
        if (!p.descriptor.keyframe) return
        decoder = make(avcCodecString(p.payload) ?? "avc1.42e01f")
      }
      decoder.decode(new EncodedVideoChunk({ type: p.descriptor.keyframe ? "key" : "delta", timestamp: p.descriptor.capture_timestamp_us ?? 0, data: p.payload }))
      const first = frames === 0
      frames = s.frames
      onFrame(frames, p.descriptor.width_px, p.descriptor.height_px, first)
    },
    reset(): void {
      decoder?.close()
      decoder = null
      frames = 0
      self.socket = null
    },
  }
  return self
}

let statsTick = 0

async function startStream(open: () => Promise<{ media: OpenedMedia; close: () => Promise<void> }>): Promise<void> {
  await stopStream()
  if (typeof VideoDecoder === "undefined") throw new Error("this browser has no WebCodecs VideoDecoder")
  state.stream.status = "Connecting to the desktop…"
  const { media, close } = await open()
  const { ws } = await attachMedia(WebSocket as never, media, 20_000, desktopPainter.feed)
  desktopPainter.socket = ws as never
  current = { ws: ws as never, close }
}

async function stopStream(): Promise<void> {
  const c = current
  current = null
  desktopPainter.reset()
  state.stream.frames = 0
  if (pip.isOpen(desktopSource)) pip.close()
  if (c) {
    c.ws.close(1000)
    await c.close().catch(() => {})
  }
}

/** Streams the selected Space through a server ticket (the spacesd token
 *  never reaches the page). */
function syncStream(): void {
  const space = state.snap.space
  const want = space && state.stream.on && space.features.includes("desktop_stream") ? space.id : null
  if (want === streamedSpace) return renderComputer()
  streamedSpace = want
  windowList = []
  pip.close()
  void refreshWindows()
  if (!want) {
    void stopStream().then(renderComputer)
    return
  }
  startStream(async () => {
    const t = await api<OpenedMedia & { needsHeaders: boolean }>("stream", { body: { maxFps: 30, maxDimension: 1280 } })
    if (t.needsHeaders) throw new Error("this Space's media socket needs gateway headers a browser cannot send")
    return { media: t, close: () => api("stream/close", { body: { mediaSessionId: t.mediaSessionId } }).then(() => {}) }
  })
    .catch((e) => {
      state.stream.status = `Stream unavailable: ${e instanceof Error ? e.message : String(e)}`
    })
    .finally(renderComputer)
}

/** Direct: the wasm gRPC-Web client opens the session itself. */
async function startDirect(url: string, envToken: string): Promise<void> {
  try {
    streamedSpace = "direct"
    await startStream(async () => {
      const sdk = await import("@trycua/cua/browser")
      await sdk.initialize()
      const env = (await sdk.Cua.embedded(sdk.CuaConfig.create({})).spacesd(url, envToken || undefined)) as unknown as JsonRpc
      const media = await openDesktopMedia(env, url, { maxFps: 30, maxDimension: 1280 })
      return { media, close: () => closeMedia(env, media.mediaSessionId) }
    })
  } catch (e) {
    fail(e)
  }
  renderComputer()
}

// Presence: join as the operator when the Space has the service; the
// cursor follows the pointer over the stream (throttled), and the others'
// cursors are drawn on it by the SDK's PresenceView, fed the server's raw
// presence events: interpolated, shaped, faded when idle, and removed when a
// heartbeat stops listing them or an agent's run ends.
const presenceEl = h("span", { class: "presence" })
const cursorHost = h("div", { class: "cursor-host" })
let presenceView: PresenceView | null = null
function drawCursors(p: { me: string | null; members?: PresenceMember[] } | null | undefined): void {
  const me = p?.me ?? null
  if (!me) {
    presenceView = null
  } else if (presenceView?.me !== me) {
    const mine = p?.members?.find((m) => m.participant.participantId === me)
    presenceView = mine ? PresenceView.from(mine.participant, p?.members ?? []) : null
  }
  if (!presenceView) cursorHost.replaceChildren()
}
// A timer rather than rAF alone, so staleness still expires in a hidden tab.
function paintCursors(): void {
  if (presenceView) cursorHost.replaceChildren(renderDrawables(presenceView.drawables(Date.now())))
}
setInterval(paintCursors, 1_000)
const frame = () => {
  paintCursors()
  requestAnimationFrame(frame)
}
requestAnimationFrame(frame)
let presenceSpace: string | null = null
async function syncPresence(): Promise<void> {
  const space = state.snap.space
  const want = space?.features.includes("presence") ? space.id : null
  if (want === presenceSpace) return
  presenceSpace = want
  presenceEl.replaceChildren()
  drawCursors(null)
  if (want) await api("presence/join", { body: { displayName: "Operator" } }).then(refreshPresence, () => {})
}
async function refreshPresence(): Promise<void> {
  const r = await api<{ me: string | null; entries: CursorView[]; cursors: CursorView[]; colors?: Record<string, string>; members?: PresenceMember[] }>("presence")
  drawCursors(r)
  applyAssignedColors(r.colors)
  presenceEl.replaceChildren(
    ...r.entries.map((m) =>
      h(
        "span",
        { class: "avatar initials", style: `background:${m.color || "#5d5d63"}`, title: `${m.displayName}${m.agent ? " (agent)" : ""}${m.participantId === r.me ? " (you)" : ""}` },
        m.displayName
          .split(/\s+/)
          .map((w) => w[0] ?? "")
          .join("")
          .slice(0, 2)
          .toUpperCase(),
      ),
    ),
  )
}
let lastCursor = 0
canvas.addEventListener("pointermove", (e) => {
  const now = performance.now()
  if (!presenceSpace || now - lastCursor < 100) return
  lastCursor = now
  const r = canvas.getBoundingClientRect()
  void api("presence/cursor", { body: { x: (e.clientX - r.left) / r.width, y: (e.clientY - r.top) / r.height } }).catch(() => {})
})

// ------------------------------------------------------------------ events

/** Avatars follow the colors the server assigned (the Bots' cursors). */
function applyAssignedColors(colors: Record<string, string> | undefined): void {
  if (colors && setAssignedColors(colors)) {
    renderSidebar()
    renderMain()
  }
}

function render(): void {
  renderSidebar()
  renderMain()
  renderComputer()
}

function connectEvents(): void {
  const proto = location.protocol === "https:" ? "wss" : "ws"
  const ws = new WebSocket(`${proto}://${location.host}/events?token=${encodeURIComponent(token)}`)
  ws.onopen = () => {
    state.connected = true
    renderSidebar()
  }
  ws.onmessage = (ev) => {
    const m = JSON.parse(String(ev.data)) as {
      type: string
      state?: Snapshot
      event?: unknown
      cursors?: CursorView[]
      colors?: Record<string, string>
    }
    if (m.type === "presence") {
      // Cursor moves redraw the pointers; joins and leaves refresh the avatars too.
      if (m.event && presenceView) presenceView.apply(m.event as unknown as PresenceEvent, Date.now())
      applyAssignedColors(m.colors)
      if ((m.event as { kind?: string } | undefined)?.kind !== "cursor_moved") void refreshPresence().catch(() => {})
    }
    if (m.type !== "state" || !m.state) return
    const spaceChanged = m.state.space?.id !== state.snap.space?.id
    state.snap = m.state
    if (spaceChanged) {
      state.selectedSpace = m.state.space?.id ?? null
      void refreshSpaces().catch(() => {})
      syncStream()
      void syncPresence()
    }
    if (m.state.presence?.colors) setAssignedColors(m.state.presence.colors)
    if (!state.selected && state.snap.bots[0]) state.selected = state.snap.bots[0].runId
    renderSidebar()
    renderMain()
  }
  ws.onclose = () => {
    state.connected = false
    renderSidebar()
    setTimeout(connectEvents, 2000)
  }
}

render()
if (!token) fail("Open the URL the server printed (it carries #token=…).")
else {
  api<{ cloud: boolean }>("config")
    .then((c) => {
      state.cloud = c.cloud
    })
    .catch(() => {})
    .finally(() => {
      connectEvents()
      refreshSpaces()
        .then(() => {
          if (params.get("sheet") === "wizard") openWizard(params.get("step") ? { step: Number(params.get("step")) } : undefined)
        })
        .catch(fail)
    })
}
