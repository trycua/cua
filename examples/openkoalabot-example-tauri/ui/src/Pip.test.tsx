// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// @vitest-environment happy-dom
// Picture in picture and the one drop zone over a fake shell: the PiP
// window's route and size, the window list, open and close through the
// `open_pip` / `close_pip` commands and the `pip-closed` event, and the
// `<cua-drop-zone>` wiring. No Tauri runtime, no Space.
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { idleDropZone } from "@trycua/cua/teleport"
import { desktopSource, pipWindowLabel, windowSource } from "@trycua/cua/spaces/pip"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const calls: Array<[string, Record<string, unknown> | undefined]> = []
let emitClosed: ((label: string) => void) | null = null
vi.mock("@tauri-apps/api/core", () => ({
  invoke: async (cmd: string, args?: Record<string, unknown>) => {
    calls.push([cmd, args])
    return undefined
  },
}))
vi.mock("@tauri-apps/api/event", () => ({
  listen: async (event: string, fn: (e: { payload: string }) => void) => {
    if (event === "pip-closed") emitClosed = (label) => fn({ payload: label })
    return () => {}
  },
}))

vi.mock("./rcdp", () => ({ attach: () => () => {} }))

const { PipView, WindowList, pipOpenSize, pipRoute, readPipRoute, usePip } = await import("./Pip")
const { DropZone, dropHits } = await import("./DropZone")

const rows = [
  { window_id: "target-1", app: "Xfce4-terminal", title: "top", width: 818, height: 483 },
  { window_id: "target-2", app: "Thunar", title: "cua - Thunar", width: 640, height: 480 },
]

let root: Root | null = null
let host: HTMLElement
beforeEach(() => {
  calls.length = 0
  host = document.createElement("div")
  document.body.append(host)
})
afterEach(() => {
  act(() => root?.unmount())
  root = null
  host.remove()
})
const render = (el: React.ReactNode) => act(() => (root ??= createRoot(host)).render(el))

describe("the PiP window's page", () => {
  it("routes carry the source and the Space; the main window has none", () => {
    const src = windowSource(rows[0])
    expect(readPipRoute(`#${pipRoute(src, "direct:127.0.0.1:3291")}`)).toEqual({ source: src, space: "direct:127.0.0.1:3291" })
    expect(readPipRoute(`#${pipRoute(desktopSource, "s")}`)).toEqual({ source: desktopSource, space: "s" })
    expect(readPipRoute("")).toBeNull()
    expect(readPipRoute("#pip=desktop")).toBeNull()
  })

  it("opens at the source's aspect, 480 points on the long edge", () => {
    expect(pipOpenSize(desktopSource, { width: 1280, height: 800 })).toEqual({ width: 480, height: 300 })
    expect(pipOpenSize(windowSource(rows[1]), { width: 1280, height: 800 }, rows[1])).toEqual({ width: 480, height: 360 })
  })
})

describe("the PiP window has no traffic lights", () => {
  it("closes from its close button and from Escape", async () => {
    const src = windowSource(rows[0])
    render(<PipView source={src} space="s" />)
    const button = host.querySelector<HTMLButtonElement>("button.pip-close")
    expect(button?.getAttribute("aria-label")).toBe("Close picture in picture")
    await act(async () => button!.click())
    await act(async () => void window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })))
    await act(async () => void window.dispatchEvent(new KeyboardEvent("keydown", { key: "a" })))
    const closes = calls.filter(([cmd]) => cmd === "close_pip")
    const label = pipWindowLabel(src)
    expect(closes).toEqual([
      ["close_pip", { label }],
      ["close_pip", { label }],
    ])
  })
})

describe("the window list", () => {
  it("is one line per window with a PiP button each", () => {
    const picked: string[] = []
    render(<WindowList windows={rows} isOpen={(s) => s.kind === "window" && s.windowId === "target-2"} onPip={(w) => picked.push(w.window_id)} />)
    const names = [...host.querySelectorAll(".win-row .win-name")].map((e) => e.textContent)
    expect(names).toEqual(["Xfce4-terminal · top", "cua - Thunar"])
    const buttons = host.querySelectorAll<HTMLButtonElement>(".win-row button")
    expect(buttons[0].getAttribute("aria-pressed")).toBe("false")
    expect(buttons[1].getAttribute("aria-pressed")).toBe("true")
    act(() => buttons[0].click())
    expect(picked).toEqual(["target-1"])
  })

  it("shows the app icon the Space gave, and nothing when it gave none", () => {
    const icon = "data:image/png;base64,iVBORw0KGgo="
    render(<WindowList windows={[{ ...rows[0], icon }, rows[1]]} isOpen={() => false} onPip={() => {}} />)
    const r = [...host.querySelectorAll(".win-row")]
    expect(r[0].firstElementChild?.tagName).toBe("IMG")
    expect(r[0].querySelector("img.win-icon")?.getAttribute("src")).toBe(icon)
    expect(r[1].querySelector("img")).toBeNull()
    expect(r[1].firstElementChild?.className).toBe("win-name")
  })

  it("renders nothing without windows", () => {
    render(<WindowList windows={[]} isOpen={() => false} onPip={() => {}} />)
    expect(host.querySelector(".win-list")).toBeNull()
  })
})

describe("usePip", () => {
  let pip: ReturnType<typeof usePip> | null = null
  function Probe({ space }: { space: string }) {
    pip = usePip(space)
    return <span>{pip.isOpen(desktopSource) ? "open" : "closed"}</span>
  }

  it("opens and closes an always-on-top window per source through the shell", async () => {
    render(<Probe space="direct:h:1" />)
    await act(async () => pip!.toggle(desktopSource, { width: 480, height: 300 }, "dev"))
    expect(calls.at(-1)).toEqual(["open_pip", { label: "pip-desktop", route: "pip=desktop&space=direct%3Ah%3A1", title: "dev", width: 480, height: 300 }])
    expect(host.textContent).toBe("open")
    const win = windowSource(rows[0])
    await act(async () => pip!.toggle(win, { width: 480, height: 283 }, "dev"))
    expect(calls.at(-1)?.[0]).toBe("open_pip")
    expect(calls.at(-1)?.[1]).toMatchObject({ label: "pip-w-target-1", title: "Xfce4-terminal · top" })
    expect(pip!.isOpen(win)).toBe(true)
    // Toggling again closes it; the user closing a window is `pip-closed`.
    await act(async () => pip!.toggle(win, { width: 480, height: 283 }, "dev"))
    expect(calls.at(-1)).toEqual(["close_pip", { label: "pip-w-target-1" }])
    act(() => emitClosed!("pip-desktop"))
    expect(host.textContent).toBe("closed")
  })

  it("two PiPs opened at once are both tracked", async () => {
    render(<Probe space="c" />)
    const win = windowSource(rows[0])
    await act(async () => {
      await Promise.all([pip!.toggle(desktopSource, { width: 480, height: 300 }, "c"), pip!.toggle(win, { width: 480, height: 283 }, "c")])
    })
    expect(pip!.isOpen(desktopSource)).toBe(true)
    expect(pip!.isOpen(win)).toBe(true)
  })

  it("switching Spaces closes the open PiPs", async () => {
    render(<Probe space="a" />)
    await act(async () => pip!.toggle(desktopSource, { width: 480, height: 300 }, "a"))
    calls.length = 0
    render(<Probe space="b" />)
    expect(calls).toEqual([["close_pip", { label: "pip-desktop" }]])
    expect(host.textContent).toBe("closed")
  })
})

describe("the one drop zone", () => {
  it("is <cua-drop-zone>: one caption, Send file… and Teleport an app…", () => {
    const got: string[] = []
    render(<DropZone spaceId="direct:h:1" status={idleDropZone} over={false} onSendFile={() => got.push("send")} onTeleportApp={() => got.push("teleport")} />)
    const zone = host.querySelector("cua-drop-zone")!
    expect(zone.getAttribute("data-space")).toBe("direct:h:1")
    const root = zone.shadowRoot!
    expect([...root.querySelectorAll(".caption")].map((e) => e.textContent)).toEqual(["Drop a file or window"])
    expect([...root.querySelectorAll("button")].map((b) => b.textContent)).toEqual(["Send file…", "Teleport an app…"])
    act(() => root.querySelector<HTMLButtonElement>('[data-act="send-file"]')!.click())
    act(() => root.querySelector<HTMLButtonElement>('[data-act="teleport-app"]')!.click())
    expect(got).toEqual(["send", "teleport"])
    render(<DropZone spaceId="direct:h:1" status={{ kind: "sending", label: "a.txt" }} over onSendFile={() => {}} onTeleportApp={() => {}} />)
    expect(root.querySelector(".zone")!.getAttribute("data-drop-target")).toBe("true")
    expect(root.querySelector('[role="status"]')!.textContent).toBe("Sending a.txt…")
  })

  it("hit-tests native drops in physical pixels", () => {
    const el = { getBoundingClientRect: () => ({ left: 100, top: 200, right: 300, bottom: 260, width: 200, height: 60 }) } as HTMLElement
    expect(dropHits(el, { x: 400, y: 460 }, 2)).toBe(true)
    expect(dropHits(el, { x: 400, y: 460 }, 1)).toBe(false)
    expect(dropHits(null, { x: 0, y: 0 }, 2)).toBe(false)
  })
})
