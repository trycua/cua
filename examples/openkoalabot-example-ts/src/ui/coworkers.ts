// Routines, group chats and presence cursors in the page. Views only: the
// server owns the SDK's RoutineStore and GroupChatStore and sends their
// state in the snapshot; the page posts what the user did.
import { cursorArt, cursorArtSvg, presenceColor, presenceTextColor, type PresenceDrawable } from "@trycua/cua/spaces/presence"
import { h, icon } from "./dom.js"

let assigned: Record<string, string> = {}

/**
 * The colors the server assigned to whoever is present, keyed by the id each
 * joined with (`PresenceEntry.principalId`): the SDK roster's view.
 */
export function assignedColorsFrom(entries: ReadonlyArray<{ principalId?: string; color: string }>): Record<string, string> {
  const out: Record<string, string> = {}
  for (const e of entries) if (e.principalId && e.color) out[e.principalId] = e.color.toLowerCase()
  return out
}

/** Replaces the assigned colors; returns whether any changed. */
export function setAssignedColors(colors: Record<string, string>): boolean {
  const a = JSON.stringify(Object.entries(assigned).sort())
  const b = JSON.stringify(Object.entries(colors).sort())
  assigned = { ...colors }
  return a !== b
}

/**
 * A Bot's avatar color, the same as its cursor: while the Bot is present on
 * the Space, the color the server assigned it (a requested color is kept
 * unless another participant already holds it); before that, its stable
 * presence color from the SDK, the one it requests when it joins.
 */
export function botAvatarColor(botID: string): string {
  return assigned[botID] ?? presenceColor(botID)
}

let koalaSrc = ""
/** The koala image every Bot avatar shows (the page's bundled asset). */
export function useBotAvatarImage(src: string): void {
  koalaSrc = src
}

/** A Bot's koala on its presence color. `size`: "", "sm" or "xs". */
export function botAvatar(botID: string, size = ""): HTMLElement {
  return h("span", { class: `avatar ${size}`, "data-bot": botID, style: `background:${botAvatarColor(botID)}` }, koalaSrc ? h("img", { src: koalaSrc, alt: "" }) : null)
}

export type Schedule =
  | { kind: "everyMinutes"; minutes: number }
  | { kind: "dailyAt"; hour: number; minute: number }
  | { kind: "weeklyOn"; weekday: number; hour: number; minute: number }

export interface RoutineView {
  id: string
  botID: string
  title: string
  prompt: string
  schedule: Schedule
  isEnabled: boolean
  label: string
  lastFiredAt?: string
  lastRunID?: string
  lastOutcome?: string
}

export interface GroupLine {
  id: string
  speaker: { kind: "human" } | { kind: "bot"; botID: string } | { kind: "system" }
  text: string
  undelivered: boolean
  reaction?: string
}

export interface GroupView {
  id: string
  title: string
  memberIDs: string[]
  membershipLabel: string
  messages: GroupLine[]
  working: string[]
}

export interface BotRef {
  id: string
  name: string
}

export interface CursorView {
  participantId: string
  displayName: string
  color: string
  agent: boolean
  cursor?: { x: number; y: number; visible: boolean }
}

export const GROUP_MIN = 2
export const GROUP_MAX = 6

function when(iso: string | undefined): string {
  if (!iso) return ""
  const d = new Date(iso)
  return d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
}

/** A sidebar section of one-line rows. */
function section(label: string, rows: HTMLElement[], empty: string, trailing?: HTMLElement): HTMLElement {
  return h(
    "div",
    { class: "cw-section" },
    h("div", { class: "section-label" }, label),
    ...(rows.length ? rows : [h("div", { class: "roster-empty" }, empty)]),
    trailing ?? null,
  )
}

function row(text: string, on: boolean, disabled: boolean, onclick: () => void, meta?: string): HTMLElement {
  return h(
    "button",
    { class: `cw-row ${on ? "on" : ""} ${disabled ? "off" : ""}`, onclick: () => onclick() },
    h("span", { class: "cw-text" }, text),
    meta ? h("span", { class: "cw-meta" }, meta) : null,
  )
}

/** Sidebar: one row per routine, `Title · Bot`. */
export function renderRoutinesSection(p: { routines: RoutineView[]; bots: BotRef[]; selectedBot: string | null; onOpen: (botID: string) => void }): HTMLElement {
  const name = (id: string) => p.bots.find((b) => b.id === id)?.name ?? id
  return section(
    "Routines",
    p.routines.map((r) => row(`${r.title} · ${name(r.botID)}`, p.selectedBot === r.botID, !r.isEnabled, () => p.onOpen(r.botID))),
    "No routines yet",
  )
}

/** Sidebar: one row per group chat, and New group chat. */
export function renderGroupsSection(p: { groups: GroupView[]; selected: string | null; botCount: number; onOpen: (id: string) => void; onNew: () => void }): HTMLElement {
  const canNew = p.botCount >= GROUP_MIN
  return section(
    "Group chats",
    p.groups.map((g) => row(g.title, p.selected === g.id, false, () => p.onOpen(g.id), `${g.memberIDs.length}`)),
    "No group chats yet",
    h(
      "button",
      { class: "cw-row cw-add", disabled: !canNew, title: canNew ? "Put Bots in one thread" : `A group needs at least ${GROUP_MIN} Bots`, onclick: () => p.onNew() },
      h("span", { class: "cw-text" }, "New group chat"),
    ),
  )
}

export interface RoutineInput {
  title: string
  prompt: string
  schedule: Schedule
}

const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"]

/** Reads the create form into a routine, or an error. */
export function readRoutineForm(v: { title: string; prompt: string; kind: string; minutes: string; time: string; weekday: string }): RoutineInput | string {
  const title = v.title.trim()
  const prompt = v.prompt.trim()
  if (!title) return "Name the routine."
  if (!prompt) return "Say what the Bot should do."
  const [hh, mm] = v.time.split(":").map((x) => Number(x))
  const hour = hh ?? NaN
  const minute = mm ?? NaN
  const timeOk = Number.isInteger(hour) && Number.isInteger(minute) && hour >= 0 && hour < 24 && minute >= 0 && minute < 60
  switch (v.kind) {
    case "everyMinutes": {
      const minutes = Number(v.minutes)
      if (!Number.isInteger(minutes) || minutes < 1) return "Every how many minutes?"
      return { title, prompt, schedule: { kind: "everyMinutes", minutes } }
    }
    case "dailyAt":
      return timeOk ? { title, prompt, schedule: { kind: "dailyAt", hour, minute } } : "Pick a time."
    case "weeklyOn": {
      const weekday = Number(v.weekday)
      if (!timeOk) return "Pick a time."
      return { title, prompt, schedule: { kind: "weeklyOn", weekday, hour, minute } }
    }
    default:
      return "Pick a schedule."
  }
}

/** A Bot's routines: the list, Run now, the enable switch, and a compact create form. */
export function renderRoutinesPanel(p: {
  bot: BotRef
  routines: RoutineView[]
  onCreate: (r: RoutineInput) => void
  onToggle: (id: string, enabled: boolean) => void
  onRun: (id: string) => void
  onDelete: (id: string) => void
  error?: string
}): HTMLElement {
  const list = h(
    "div",
    { class: "cw-list" },
    ...p.routines.map((r) =>
      h(
        "div",
        { class: `cw-item ${r.isEnabled ? "" : "off"}`, "data-routine": r.id },
        h("input", { type: "checkbox", checked: r.isEnabled, title: r.isEnabled ? "Turn off" : "Turn on", onchange: (e) => p.onToggle(r.id, (e.target as HTMLInputElement).checked) }),
        h("span", { class: "cw-text", title: r.prompt }, r.title),
        h("span", { class: "cw-meta", title: r.lastOutcome ?? "" }, r.lastFiredAt ? `${r.label} · last ${when(r.lastFiredAt)}` : r.label),
        h("button", { class: "btn btn-ghost", "data-action": "run", onclick: () => p.onRun(r.id) }, "Run now"),
        h("button", { class: "icon-btn", title: "Delete", "data-action": "delete", onclick: () => p.onDelete(r.id) }, icon("close")),
      ),
    ),
  )
  const title = h("input", { class: "input", placeholder: "Name", "aria-label": "Routine name" }) as HTMLInputElement
  const prompt = h("input", { class: "input", placeholder: `What ${p.bot.name} should do`, "aria-label": "Prompt" }) as HTMLInputElement
  const kind = h(
    "select",
    { class: "select", "aria-label": "Schedule" },
    h("option", { value: "everyMinutes" }, "Every"),
    h("option", { value: "dailyAt" }, "Daily at"),
    h("option", { value: "weeklyOn" }, "Weekly on"),
  ) as HTMLSelectElement
  const minutes = h("input", { class: "input cw-num", type: "number", min: 1, value: "60", "aria-label": "Minutes" }) as HTMLInputElement
  const unit = h("span", { class: "cw-unit" }, "minutes")
  const weekday = h("select", { class: "select", "aria-label": "Weekday" }, ...WEEKDAYS.map((d, i) => h("option", { value: String(i + 1) }, d))) as HTMLSelectElement
  weekday.value = "2"
  const time = h("input", { class: "input cw-time", type: "time", value: "09:00", "aria-label": "Time" }) as HTMLInputElement
  const err = h("span", { class: "cw-error" }, p.error ?? "")
  const sync = () => {
    const k = kind.value
    minutes.hidden = unit.hidden = k !== "everyMinutes"
    weekday.hidden = k !== "weeklyOn"
    time.hidden = k === "everyMinutes"
  }
  kind.addEventListener("change", sync)
  sync()
  const add = h("button", {
    class: "btn btn-primary",
    "data-action": "create",
    onclick: () => {
      const r = readRoutineForm({ title: title.value, prompt: prompt.value, kind: kind.value, minutes: minutes.value, time: time.value, weekday: weekday.value })
      if (typeof r === "string") {
        err.textContent = r
        return
      }
      err.textContent = ""
      p.onCreate(r)
    },
  }, "Add")
  return h(
    "div",
    { class: "cw-panel" },
    p.routines.length ? list : h("p", { class: "cw-hint" }, `Routines are recurring tasks ${p.bot.name} runs on a schedule.`),
    h("div", { class: "cw-form" }, title, prompt, h("div", { class: "cw-when" }, kind, minutes, unit, weekday, time, h("span", { class: "spacer" }), add), err),
  )
}

/** New group chat: pick 2 to 6 Bots. Create is disabled outside the bound. */
export function renderNewGroupSheet(p: { bots: BotRef[]; onCancel: () => void; onCreate: (title: string, members: string[]) => void }): HTMLElement {
  const picked = new Set<string>()
  const title = h("input", { class: "input", placeholder: "Optional", "aria-label": "Group name" }) as HTMLInputElement
  const count = h("span", { class: "cw-meta" }, `0 of ${GROUP_MAX}`)
  const create = h("button", { class: "btn btn-primary", disabled: true, "data-action": "create" }, "Create") as HTMLButtonElement
  const update = () => {
    count.textContent = `${picked.size} of ${GROUP_MAX}`
    create.disabled = picked.size < GROUP_MIN || picked.size > GROUP_MAX
    for (const box of boxes) box.disabled = !box.checked && picked.size >= GROUP_MAX
  }
  const boxes: HTMLInputElement[] = []
  const list = h(
    "div",
    { class: "cw-list" },
    ...p.bots.map((b) => {
      const box = h("input", {
        type: "checkbox",
        value: b.id,
        onchange: (e) => {
          const on = (e.target as HTMLInputElement).checked
          if (on) picked.add(b.id)
          else picked.delete(b.id)
          update()
        },
      }) as HTMLInputElement
      boxes.push(box)
      return h("label", { class: "cw-item" }, box, botAvatar(b.id, "xs"), h("span", { class: "cw-text" }, b.name))
    }),
  )
  create.addEventListener("click", () => p.onCreate(title.value.trim(), [...picked]))
  return h(
    "div",
    { class: "scrim", role: "dialog", "aria-modal": "true", "aria-label": "New group chat" },
    h(
      "div",
      { class: "sheet sm" },
      h("div", { class: "sheet-head" }, h("h2", {}, "New group chat")),
      h("div", { class: "sheet-body" }, h("label", { class: "field" }, h("span", {}, "Name"), title), h("div", { class: "field" }, h("span", {}, "Bots ", count), list)),
      h("div", { class: "sheet-foot" }, h("span", { class: "spacer" }), h("button", { class: "btn", "data-action": "cancel", onclick: () => p.onCancel() }, "Cancel"), create),
    ),
  )
}

/** A group thread: the attributed transcript and the typing row. */
export function renderGroupThread(p: { group: GroupView; names: Record<string, string> }): HTMLElement {
  const name = (id: string) => p.names[id] ?? id
  const col = h("div", { class: "col" })
  for (const l of p.group.messages) {
    if (l.speaker.kind === "human") col.append(h("div", { class: "msg msg-user" }, h("div", { class: "bubble" }, l.text)))
    else if (l.speaker.kind === "system") col.append(h("div", { class: `cw-system ${l.undelivered ? "undelivered" : ""}` }, l.text))
    else
      col.append(
        h(
          "div",
          { class: `cw-line ${l.undelivered ? "undelivered" : ""}`, "data-bot": l.speaker.botID },
          h("div", { class: "author" }, botAvatar(l.speaker.botID, "xs"), name(l.speaker.botID)),
          h("div", { class: "text" }, l.text),
          l.reaction ? h("span", { class: "cw-reaction" }, l.reaction) : null,
        ),
      )
  }
  if (p.group.working.length) col.append(h("div", { class: "cw-typing" }, h("span", { class: "spinner" }), `${p.group.working.map(name).join(", ")} ${p.group.working.length === 1 ? "is" : "are"} working`))
  return h("div", { class: "scroll" }, col)
}

/** The other participants' cursors, as pointers over the stream (never mine). */
export function renderCursors(cursors: CursorView[], me: string | null): HTMLElement {
  const layer = h("div", { class: "cursors", "aria-hidden": "true" })
  for (const c of cursors) {
    if (c.participantId === me || !c.cursor || !c.cursor.visible) continue
    const x = Math.min(1, Math.max(0, c.cursor.x)) * 100
    const y = Math.min(1, Math.max(0, c.cursor.y)) * 100
    const ns = "http://www.w3.org/2000/svg"
    const svg = document.createElementNS(ns, "svg")
    svg.setAttribute("viewBox", "0 0 16 16")
    svg.setAttribute("width", "16")
    svg.setAttribute("height", "16")
    const path = document.createElementNS(ns, "path")
    path.setAttribute("d", "M2 1.5v12l3.4-3.1 2.3 4.9 2-.9-2.3-4.8H12z")
    path.setAttribute("fill", c.color || "#3b82f6")
    path.setAttribute("stroke", "#fff")
    path.setAttribute("stroke-width", "1")
    svg.append(path)
    layer.append(
      h(
        "div",
        { class: "cursor", "data-participant": c.participantId, style: `left:${x.toFixed(2)}%;top:${y.toFixed(2)}%` },
        svg,
        h("span", { class: "cursor-name", style: `background:${c.color || "#3b82f6"}` }, c.displayName),
      ),
    )
  }
  return layer
}

/** Drawn size of a presence cursor (the shared art is a 32 px canvas). */
export const CURSOR_PX = 20

/**
 * The presence cursors to draw now, from the SDK's `PresenceView.drawables`:
 * interpolated positions, the guest's cursor shape in the shared art, the
 * participant's color and name, and the idle fade. Never mine (the pointer
 * is the operator's own cursor).
 */
export function renderDrawables(drawables: readonly PresenceDrawable[]): HTMLElement {
  const layer = h("div", { class: "cursors", "aria-hidden": "true" })
  for (const d of drawables) {
    if (d.isMe || d.alpha <= 0) continue
    const art = cursorArt(d.shape)
    const k = CURSOR_PX / 32
    const x = Math.min(1, Math.max(0, d.x)) * 100
    const y = Math.min(1, Math.max(0, d.y)) * 100
    const glyph = h("span", { class: "cursor-art", style: `width:${CURSOR_PX}px;height:${CURSOR_PX}px` })
    glyph.innerHTML = cursorArtSvg(d.shape, d.color || "#3b82f6", CURSOR_PX)
    layer.append(
      h(
        "div",
        {
          class: "cursor",
          "data-participant": d.participantId,
          "data-shape": d.shape,
          style: `left:${x.toFixed(2)}%;top:${y.toFixed(2)}%;opacity:${d.alpha.toFixed(2)};transform:translate(${-art.hotspot[0] * k}px, ${-art.hotspot[1] * k}px)`,
        },
        glyph,
        h("span", { class: "cursor-name", style: `background:${d.color || "#3b82f6"};color:${presenceTextColor(d.color || "#3b82f6")}` }, d.displayName),
      ),
    )
  }
  return layer
}
