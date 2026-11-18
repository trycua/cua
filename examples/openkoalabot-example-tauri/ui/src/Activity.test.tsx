// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// @vitest-environment happy-dom
import { act } from "react"
import { createRoot } from "react-dom/client"
import { describe, expect, it } from "vitest"
import { ActivityGroup } from "./Activity"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

describe("an activity group", () => {
  it("is one muted summary line, collapsed, and lists its steps when opened", () => {
    const host = document.createElement("div")
    document.body.append(host)
    const root = createRoot(host)
    act(() => root.render(<ActivityGroup summary="2 steps" steps={["Install node: cached", "Turn 1 ended (end_turn)"]} />))
    const head = host.querySelector<HTMLButtonElement>(".activity-head")!
    expect(head.textContent).toContain("2 steps")
    expect(head.getAttribute("aria-expanded")).toBe("false")
    expect(host.querySelectorAll(".activity-step")).toHaveLength(0)
    expect(host.querySelector(".bubble, .card")).toBeNull()
    act(() => head.click())
    expect([...host.querySelectorAll(".activity-step")].map((e) => e.textContent)).toEqual([
      "Install node: cached",
      "Turn 1 ended (end_turn)",
    ])
    act(() => root.unmount())
    host.remove()
  })
})
