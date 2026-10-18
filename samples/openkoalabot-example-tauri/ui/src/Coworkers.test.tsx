// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// @vitest-environment happy-dom
import { act } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, describe, expect, it, vi } from "vitest"
import type { Avatar, GroupView, RoutinesView } from "./api"
import { agentIdentity, presenceColor } from "@trycua/cua/spaces/presence"
import {
  BotAvatar,
  GroupThread,
  NewGroupSheet,
  RemoteCursors,
  RoutinesPanel,
  assignedColorsFrom,
  botAvatarColor,
  botPreview,
  contentRect,
  scheduleFrom,
  setAssignedColors,
} from "./Coworkers"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function mount(node: React.ReactNode) {
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  act(() => root.render(node))
  return host
}
const click = (el: Element | null | undefined) => act(() => (el as HTMLElement).click())
function type(el: Element | null, value: string) {
  act(() => {
    const input = el as HTMLInputElement
    const proto = input instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype
    Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(input, value)
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
}
const button = (host: Element, text: string) => [...host.querySelectorAll("button")].find((b) => b.textContent?.trim() === text)

afterEach(() => {
  document.body.innerHTML = ""
})

const view: RoutinesView = {
  routines: [
    {
      id: "r1",
      botID: "ada",
      title: "Sweep",
      prompt: "Triage",
      schedule: { kind: "dailyAt", hour: 8, minute: 0 },
      isEnabled: true,
      createdAt: "2026-09-25T09:00:00Z",
      lastOutcome: "started run run-1",
      label: "Every day at 8:00 AM",
    },
    { id: "r2", botID: "bo", title: "Other", prompt: "x", schedule: { kind: "everyMinutes", minutes: 5 }, isEnabled: false, createdAt: "2026-09-25T09:00:00Z", label: "" },
  ],
  log: [],
}

describe("routines", () => {
  it("schedules come from the form fields, and invalid ones are refused", () => {
    expect(scheduleFrom("everyMinutes", 15, "", 1)).toEqual({ kind: "everyMinutes", minutes: 15 })
    expect(scheduleFrom("everyMinutes", 0, "", 1)).toBeUndefined()
    expect(scheduleFrom("dailyAt", 0, "08:05", 1)).toEqual({ kind: "dailyAt", hour: 8, minute: 5 })
    expect(scheduleFrom("weeklyOn", 0, "13:00", 2)).toEqual({ kind: "weeklyOn", weekday: 2, hour: 13, minute: 0 })
    expect(scheduleFrom("dailyAt", 0, "", 1)).toBeUndefined()
  })

  it("lists only this Bot's routines, one line each, and acts on them", () => {
    const onRun = vi.fn()
    const onEnable = vi.fn()
    const onDelete = vi.fn()
    const host = mount(<RoutinesPanel bot={{ id: "ada", name: "Ada" }} view={view} onCreate={() => {}} onEnable={onEnable} onRun={onRun} onDelete={onDelete} />)
    const rows = host.querySelectorAll(".group-list:not(.form) .list-row")
    expect(rows).toHaveLength(1)
    expect(rows[0]!.textContent).toContain("Sweep")
    expect(rows[0]!.textContent).toContain("Every day at 8:00 AM")
    click(button(host, "Run now"))
    expect(onRun).toHaveBeenCalledWith("r1")
    click(host.querySelector('input[type="checkbox"]'))
    expect(onEnable).toHaveBeenCalledWith("r1", false)
    click(host.querySelector('button[title="Delete"]'))
    expect(onDelete).toHaveBeenCalledWith("r1")
  })

  it("creates a routine only with a title, a prompt and a valid schedule", () => {
    const onCreate = vi.fn()
    const host = mount(<RoutinesPanel bot={{ id: "cy", name: "Cy" }} view={view} onCreate={onCreate} onEnable={() => {}} onRun={() => {}} onDelete={() => {}} />)
    expect(host.textContent).toContain("No routines yet")
    const add = button(host, "Add routine")!
    expect(add.disabled).toBe(true)
    type(host.querySelector('input[aria-label="Title"]'), "Standup")
    type(host.querySelector('textarea[aria-label="Prompt"]'), "Post a status line")
    expect(add.disabled).toBe(false)
    click(add)
    expect(onCreate).toHaveBeenCalledWith("Standup", "Post a status line", { kind: "dailyAt", hour: 9, minute: 0 })
  })
})

describe("group chats", () => {
  const bots = ["a", "b", "c", "d", "e", "f", "g"].map((id) => ({ id, name: id.toUpperCase() }))

  it("Create is disabled outside 2 to 6 Bots; a seventh cannot be picked", () => {
    const onCreate = vi.fn()
    const host = mount(<NewGroupSheet bots={bots} onCancel={() => {}} onCreate={onCreate} />)
    type(host.querySelector("input.input"), "Launch")
    const create = button(host, "Create")!
    const boxes = [...host.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')]
    click(boxes[0])
    expect(create.disabled).toBe(true)
    click(boxes[1])
    expect(create.disabled).toBe(false)
    for (const b of boxes.slice(2, 6)) click(b)
    expect(boxes[6]!.disabled).toBe(true)
    click(create)
    expect(onCreate).toHaveBeenCalledWith("Launch", ["a", "b", "c", "d", "e", "f"])
  })

  it("the transcript is attributed, undelivered lines are marked, and the typing row names who works", () => {
    const chat: GroupView = {
      id: "g",
      title: "Launch",
      memberIDs: ["a", "b"],
      createdAt: "",
      label: "2 of 6 bots",
      working: ["b"],
      messages: [
        { id: "1", speaker: { kind: "human" }, text: "Status?", at: "", undelivered: false },
        { id: "2", speaker: { kind: "bot", botID: "a" }, text: "shipped", at: "", undelivered: false },
        { id: "3", speaker: { kind: "bot", botID: "b" }, text: "Did not receive that message: mid-turn", at: "", undelivered: true },
      ],
    }
    const onSend = vi.fn()
    const host = mount(<GroupThread chat={chat} name={(id) => id.toUpperCase()} onSend={onSend} />)
    const lines = host.querySelectorAll(".group-line")
    expect(lines[1]!.textContent).toBe("Ashipped")
    expect(lines[2]!.classList.contains("undelivered")).toBe(true)
    expect(host.textContent).toContain("B working")
    const ta = host.querySelector("textarea")!
    type(ta, "Go")
    act(() => ta.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })))
    expect(onSend).toHaveBeenCalledWith("Go")
  })
})

describe("presence cursors", () => {
  it("maps into the letterboxed picture", () => {
    expect(contentRect({ width: 1600, height: 900 }, { left: 0, top: 0, width: 800, height: 500 })).toEqual({ left: 0, top: 25, width: 800, height: 450 })
  })

  it("draws the others' cursors with their names, never mine", () => {
    const canvas = document.createElement("canvas")
    canvas.width = 800
    canvas.height = 500
    Object.defineProperty(canvas, "clientWidth", { value: 800 })
    Object.defineProperty(canvas, "clientHeight", { value: 500 })
    const avatars: Avatar[] = [
      { participant_id: "me", principal_id: "operator", display_name: "Operator", color: "#111111", agent: false, cursor: [0.5, 0.5], me: true },
      { participant_id: "k", principal_id: "koala", display_name: "Koala", color: "#22aa55", agent: true, cursor: [0.25, 0.75], me: false },
      { participant_id: "n", principal_id: "nobody", display_name: "Nobody", color: "", agent: false, cursor: null, me: false },
    ]
    const host = mount(<RemoteCursors avatars={avatars} canvas={canvas} />)
    const cursors = host.querySelectorAll<HTMLElement>(".cursor")
    expect(cursors).toHaveLength(1)
    expect(cursors[0]!.textContent).toBe("Koala")
    // The arrow's hot spot (2, 2 of 32, drawn at 20 px) sits on the point.
    expect(cursors[0]!.style.left).toBe("198.75px")
    expect(cursors[0]!.style.top).toBe("373.75px")
    expect(cursors[0]!.dataset.shape).toBe("arrow")
  })

  it("draws the guest's shape in the participant's color, faded when idle, and nothing once faded out", () => {
    const canvas = document.createElement("canvas")
    Object.defineProperty(canvas, "clientWidth", { value: 800 })
    Object.defineProperty(canvas, "clientHeight", { value: 500 })
    const base = { principal_id: "p", agent: true, me: false } as const
    const avatars: Avatar[] = [
      { ...base, participant_id: "t", display_name: "Typing", color: "#22aa55", cursor: [0.5, 0.5], shape: "text", alpha: 0.5 },
      { ...base, participant_id: "g", display_name: "Gone", color: "#22aa55", cursor: [0.5, 0.5], shape: "pointer", alpha: 0 },
    ]
    const host = mount(<RemoteCursors avatars={avatars} canvas={canvas} />)
    const cursors = host.querySelectorAll<HTMLElement>(".cursor")
    expect(cursors).toHaveLength(1)
    expect(cursors[0]!.dataset.shape).toBe("text")
    expect(cursors[0]!.style.opacity).toBe("0.5")
    expect(cursors[0]!.querySelector(".cursor-art")!.innerHTML).toContain('fill="#22aa55"')
  })
})

describe("avatar colors", () => {
  /** The browser's own spelling of a color, so hex and rgb() compare. */
  const norm = (c: string) => {
    const el = document.createElement("span")
    el.style.color = c
    return el.style.color
  }

  it("a Bot's avatar background is its presence cursor's color", () => {
    const canvas = document.createElement("canvas")
    Object.defineProperty(canvas, "clientWidth", { value: 800 })
    Object.defineProperty(canvas, "clientHeight", { value: 500 })
    for (const botID of ["ada", "bo", "openkoalabots-koala-1"]) {
      // The cursor of a participant joined with the Bot's SDK agent identity.
      const who = agentIdentity(botID, botID)
      const cursorHost = mount(
        <RemoteCursors avatars={[{ participant_id: "p", principal_id: botID, display_name: botID, color: who.color, agent: true, cursor: [0.5, 0.5], me: false }]} canvas={canvas} />,
      )
      const cursorColor = cursorHost.querySelector<HTMLElement>(".cursor")!.style.color
      // The avatar in the Bot list, headers and thread, in group attribution and in the picker.
      const own = mount(<BotAvatar botId={botID} />).querySelector<HTMLElement>(".avatar")!
      const chat: GroupView = {
        id: "g",
        title: "t",
        memberIDs: [botID, "x"],
        createdAt: "2026-09-25T00:00:00Z",
        label: "2 of 6 bots",
        working: [],
        messages: [{ id: "1", speaker: { kind: "bot", botID }, text: "hi", at: "2026-09-25T00:00:00Z", undelivered: false }],
      }
      const thread = mount(<GroupThread chat={chat} name={(id) => id} onSend={() => {}} />)
      const inGroup = thread.querySelector<HTMLElement>(`.avatar[data-bot="${botID}"]`)!
      const sheet = mount(<NewGroupSheet bots={[{ id: botID, name: botID }]} onCancel={() => {}} onCreate={() => {}} />)
      const inPicker = sheet.querySelector<HTMLElement>(`.avatar[data-bot="${botID}"]`)!
      for (const a of [own, inGroup, inPicker]) {
        expect(a).toBeTruthy()
        expect(norm(a.style.background || a.style.backgroundColor)).toBe(cursorColor)
      }
      expect(cursorColor).toBe(norm(presenceColor(botID)))
      expect(botAvatarColor(botID)).toBe(presenceColor(botID))
      // Chat lines stay neutral.
      expect(thread.querySelector(".bubble-bot")).toBeNull()
    }
  })
})

describe("the server-assigned color is the Bot's", () => {
  it("follows the cursor color the server assigned while the Bot is present", () => {
    const stable = presenceColor("ada")
    // The operator already holds Ada's stable color, so the server gave Ada another.
    const avatars: Avatar[] = [
      { participant_id: "me", principal_id: "operator", display_name: "Operator", color: stable, agent: false, cursor: null, me: true },
      { participant_id: "p-ada", principal_id: "ada", display_name: "Ada", color: "#123456", agent: true, cursor: [0.5, 0.5], me: false },
    ]
    try {
      expect(setAssignedColors(assignedColorsFrom(avatars))).toBe(true)
      const host = mount(
        <>
          <BotAvatar botId="ada" />
          <RemoteCursors avatars={avatars} canvas={null} />
        </>,
      )
      expect(botAvatarColor("ada")).toBe("#123456")
      const bg = (host.querySelector('.avatar[data-bot="ada"]') as HTMLElement).style.background
      expect(bg.replace(/\s/g, "")).toMatch(/#123456|rgb\(18,52,86\)/)
      expect(setAssignedColors(assignedColorsFrom(avatars))).toBe(false)
      // Gone from the roster: the stable color again.
      setAssignedColors(assignedColorsFrom(avatars.slice(0, 1)))
      expect(botAvatarColor("ada")).toBe(stable)
    } finally {
      setAssignedColors({})
    }
  })
})

describe("the sidebar preview", () => {
  const group = (messages: GroupView["messages"]): GroupView => ({ id: "g", title: "Launch", memberIDs: ["ada", "bo"], createdAt: "", messages, label: "2 of 6 bots", working: [] })
  it("shows a Bot's latest group reply when its own thread is empty", () => {
    const g = group([
      { id: "1", speaker: { kind: "human" }, text: "Where are we?", at: "2026-09-25T10:00:00Z", undelivered: false },
      { id: "2", speaker: { kind: "bot", botID: "ada" }, text: "Drafting.", at: "2026-09-25T10:00:05Z", undelivered: false },
      { id: "3", speaker: { kind: "bot", botID: "ada" }, text: "Shipped.", at: "2026-09-25T10:01:00Z", undelivered: false },
      { id: "4", speaker: { kind: "bot", botID: "ada" }, text: "Did not receive that message: busy", at: "2026-09-25T10:02:00Z", undelivered: true },
    ])
    expect(botPreview("ada", "", [g])).toBe("Launch: Shipped.")
    expect(botPreview("bo", "", [g])).toBe("")
    expect(botPreview("ada", "You: hi", [g])).toBe("You: hi")
  })
})
