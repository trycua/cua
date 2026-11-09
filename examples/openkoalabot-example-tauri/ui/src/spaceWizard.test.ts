// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest"
import { imageByRef, publishedImages } from "./images"
import { back, blocker, initialState, isDnsLabel, next, pickImage, pickOs, pickTarget, summaryRows, supports, toPlan } from "./spaceWizard"

const linux = "ghcr.io/trycua/linux:24.04"
const mac = publishedImages().find((i) => i.os === "macos")!

describe("the New Space wizard", () => {
  it("starts on System with the first published image", () => {
    const s = initialState(true)
    expect(s.step).toBe(0)
    expect(s.image).toBe(publishedImages()[0].ref)
    expect(isDnsLabel(s.name)).toBe(true)
  })

  it("disables what the image does not support", () => {
    expect(mac.cloud).toBeNull()
    expect(supports(mac, "cloud")).toBe(false)
    let s = pickTarget(initialState(true), "cloud")
    s = pickImage(s, mac.ref)
    expect(s.target).toBe("local") // switched: no cloud for this image
    expect(pickTarget(s, "cloud").target).toBe("local")
  })

  it("an OS tile picks that OS's first image", () => {
    const s = pickOs(initialState(false), "windows")
    expect(imageByRef(s.image)!.os).toBe("windows")
  })

  it("walks the four steps and refuses a bad name on Options", () => {
    let s = initialState(false)
    s = next(next(s))
    expect(s.step).toBe(2)
    s = { ...s, name: "Not A Label" }
    expect(blocker(s)).toMatch(/lowercase/)
    expect(next(s).step).toBe(2)
    s = next({ ...s, name: "koala-1" })
    expect(s.step).toBe(3)
    expect(next(s).step).toBe(3)
    expect(back(s).step).toBe(2)
  })

  it("plans a local Space with resources and a cloud Space without", () => {
    const local = { ...pickTarget(pickImage(initialState(false), linux), "local"), cpus: 6, memoryMb: 12288, name: "desk" }
    expect(toPlan(local)).toEqual({ image: linux, target: "local", name: "desk", cpus: 6, memory_mb: 12288 })
    const cloud = { ...pickTarget(local, "cloud") }
    expect(toPlan(cloud)).toEqual({ image: linux, target: "cloud", name: "desk", cpus: null, memory_mb: null })
  })

  it("summarises the runtime", () => {
    const rows = Object.fromEntries(summaryRows(pickTarget(pickImage(initialState(true), linux), "cloud")))
    expect(rows.Runtime).toBe("gVisor sandbox")
    expect(rows.Runs).toBe("Cua Cloud (metered)")
  })
})
