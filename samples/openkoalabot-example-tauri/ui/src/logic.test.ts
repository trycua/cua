// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest"
import { decide, formatBytes, groupByTurn, initials, normalizeSpaceUrl, type TeleportManifest } from "./logic"

const manifest: TeleportManifest = {
  app: "firefox",
  display_name: "Firefox",
  total_estimated_bytes: 10,
  notes: [],
  items: [
    { relative_path: "prefs.js", label: "Preferences", estimated_bytes: 1, is_sensitive: false, is_checked_by_default: true },
    { relative_path: "cookies.sqlite", label: "Cookies", estimated_bytes: 2, is_sensitive: true, is_checked_by_default: true },
    { relative_path: "places.sqlite", label: "History", estimated_bytes: 3, is_sensitive: false, is_checked_by_default: false },
  ],
}

describe("normalizeSpaceUrl", () => {
  it("adds a scheme to host:port and keeps URLs and space ids", () => {
    expect(normalizeSpaceUrl(" 10.0.0.5:3211 ")).toBe("http://10.0.0.5:3211")
    expect(normalizeSpaceUrl("https://box.example/")).toBe("https://box.example")
    expect(normalizeSpaceUrl("space://direct/10.0.0.5:3211")).toBe("space://direct/10.0.0.5:3211")
    expect(normalizeSpaceUrl("direct:10.0.0.5:3211")).toBe("direct:10.0.0.5:3211")
    expect(normalizeSpaceUrl("local:box")).toBe("local:box")
    expect(normalizeSpaceUrl("localhost:3211")).toBe("http://localhost:3211")
  })
  it("refuses what spaces.add cannot use", () => {
    expect(normalizeSpaceUrl("")).toBeNull()
    expect(normalizeSpaceUrl("justahost")).toBeNull()
    expect(normalizeSpaceUrl("ftp://x:1")).toBeNull()
  })
})

describe("decide (teleport approval)", () => {
  it("needs an acknowledgement before sensitive items move", () => {
    const r = decide(manifest, new Set(["prefs.js", "cookies.sqlite"]), false)
    expect(r.decision).toBeNull()
    expect(r.reason).toMatch(/sensitive/)
  })
  it("sends null include for exactly the defaults", () => {
    const r = decide(manifest, new Set(["prefs.js", "cookies.sqlite"]), true)
    expect(r.decision).toEqual({ include: null, acknowledge_sensitive: true })
  })
  it("names an explicit selection, sorted", () => {
    const r = decide(manifest, new Set(["places.sqlite", "prefs.js"]), false)
    expect(r.decision).toEqual({ include: ["places.sqlite", "prefs.js"], acknowledge_sensitive: false })
  })
  it("refuses an empty selection", () => {
    expect(decide(manifest, new Set(), true).decision).toBeNull()
  })
})

describe("view helpers", () => {
  it("groups transcript lines by turn", () => {
    const g = groupByTurn([
      { turn: 1, speaker: "user", text: "a" },
      { turn: 1, speaker: "activity", text: "1 step", steps: ["Install node: cached"] },
      { turn: 2, speaker: "user", text: "c" },
    ])
    expect(g.map((x) => [x.turn, x.lines.length])).toEqual([[1, 2], [2, 1]])
  })
  it("makes initials and byte labels", () => {
    expect(initials("Koala")).toBe("KO")
    expect(initials("Ada Love Lace")).toBe("AL")
    expect(initials("  ")).toBe("?")
    expect(formatBytes(512)).toBe("512 B")
    expect(formatBytes(2048)).toBe("2.0 KB")
    expect(formatBytes(3 * 1024 * 1024)).toBe("3.0 MB")
  })
})

import { APP_TELEPORT_IN_CUA_SPACES, friendlyError } from "./logic"

describe("friendlyError", () => {
  it("says a rejected token in words and keeps the detail", () => {
    const f = friendlyError("status: Unauthenticated, message: \"missing or invalid bearer token\"")
    expect(f.message).toBe("The Space rejected the token. Check the token and try again.")
    expect(f.detail).toContain("missing or invalid bearer token")
  })
  it("passes other errors through", () => {
    expect(friendlyError("boom")).toEqual({ message: "boom", detail: "" })
  })
})

describe("friendlyError: Install Cua", () => {
  it("turns the Keyvault's requires_cua_app refusal into the install affordance", () => {
    const f = friendlyError("teleport refused: requires_cua_app: teleport goes through the Cua Keyvault, which needs the Cua app")
    expect(f.message).toBe(
      "Install Cua to teleport your session. The Cua app keeps your logins in its Keyvault and asks you before sharing them.",
    )
    expect(f.link).toEqual({ label: "Install Cua", url: "https://cua.ai/install" })
    expect(f.detail).toContain("requires_cua_app")
    expect(friendlyError("teleport refused: denied").link).toBeUndefined()
  })
})

describe("friendlyError: teleport ships with Cua Spaces", () => {
  it("says so plainly when the in-process runtime refuses teleport", () => {
    const raw =
      "teleport is not available on this host: teleport ships with Cua Spaces (source-available, FSL-1.1-MIT); connect to the daemon Cua Spaces runs (`cua daemon` from the Cua Spaces app) or register the Cua Spaces extensions in this process"
    const f = friendlyError(raw)
    expect(f.message).toBe("Session teleport ships with Cua Spaces (source-available).")
    expect(f.detail).toBe(raw)
    expect(f.link).toBeUndefined()
    expect(APP_TELEPORT_IN_CUA_SPACES).toBe("App teleport ships with Cua Spaces (source-available).")
  })
})
