// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { FIXTURE_SPACES } from "./fixtures";
import { filterSpaces, osIcon, osIconUrl } from "./notch";

describe("notch model (the core's)", () => {
  it("picks the OS icon, a distro's own mark when reported", () => {
    expect(osIcon("macos")).toBe("os-macos");
    expect(osIcon("windows", "Windows 11 Pro")).toBe("os-windows");
    expect(osIcon("linux", "Ubuntu 24.04")).toBe("os-ubuntu");
    expect(osIcon("linux")).toBe("os-linux");
    expect(osIconUrl("os-ubuntu")).toMatch(/^data:image\/svg\+xml/);
    expect(osIconUrl("nope")).toBeNull();
  });

  it("filters by name, OS and status, keeping order", () => {
    const win = filterSpaces(FIXTURE_SPACES, "windows");
    expect(win.length).toBeGreaterThan(0);
    expect(win.every((s) => s.os === "windows" || /windows/i.test(s.name + s.detail))).toBe(true);
    expect(filterSpaces(FIXTURE_SPACES, "")).toEqual(FIXTURE_SPACES);
    expect(filterSpaces(FIXTURE_SPACES, "zzz-no-match")).toEqual([]);
  });
});
