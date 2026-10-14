// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// @vitest-environment happy-dom
import { act } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, describe, expect, it, vi } from "vitest"
import type { SpaceInfo } from "./api"
import { SpaceSection } from "./SpaceSection"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const spaces: SpaceInfo[] = [
  { id: "cloud:desk", name: "desk", provider: "cloud", spacesd_version: "1", features: [] },
  { id: "direct:10.0.0.5:3211", name: "lab", provider: "direct", spacesd_version: "1", features: [] },
]

function mount(onSpace = vi.fn(), onDelete = vi.fn()) {
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  act(() => root.render(<SpaceSection spaces={spaces} space={spaces[0]} onSpace={onSpace} onNewSpace={() => {}} onDelete={onDelete} />))
  return { host, root, onSpace, onDelete }
}

const click = (el: Element | null) => act(() => (el as HTMLElement).click())

afterEach(() => {
  document.body.innerHTML = ""
})

describe("the Space section", () => {
  it("picking a Space selects it and never deletes", () => {
    const { host, onSpace, onDelete } = mount()
    const select = host.querySelector("select")!
    act(() => {
      select.value = spaces[1].id
      select.dispatchEvent(new Event("change", { bubbles: true }))
    })
    click(select)
    expect(onSpace).toHaveBeenCalledWith(spaces[1].id)
    expect(onDelete).not.toHaveBeenCalled()
  })

  it("Delete asks first; Cancel deletes nothing", () => {
    const { host, onDelete } = mount()
    click(host.querySelector('[data-action="delete"]'))
    expect(onDelete).not.toHaveBeenCalled()
    expect(host.textContent).toContain("Delete desk?")
    expect(host.textContent).toContain("deletes the Cua Cloud Space")
    click(host.querySelector('[data-action="cancel"]'))
    expect(host.querySelector('[role="dialog"]')).toBeNull()
    expect(onDelete).not.toHaveBeenCalled()
  })

  it("deletes only after the confirmation", () => {
    const { host, onDelete } = mount()
    click(host.querySelector('[data-action="delete"]'))
    click(host.querySelector('[data-action="confirm-delete"]'))
    expect(onDelete).toHaveBeenCalledTimes(1)
    expect(onDelete).toHaveBeenCalledWith("cloud:desk")
  })
})
