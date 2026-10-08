// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { hostOs, osOfPlatform, showsMacShellSettings, thisComputerLabel, thisComputerText } from "./host-labels";

describe("host labels", () => {
  it("names the local machine after its system", () => {
    expect(thisComputerLabel("MacIntel")).toBe("This Mac");
    expect(thisComputerLabel("darwin")).toBe("This Mac");
    expect(thisComputerLabel("Win32")).toBe("This PC");
    expect(thisComputerLabel("win32")).toBe("This PC");
    expect(thisComputerLabel("Linux x86_64")).toBe("This computer");
    expect(thisComputerLabel("linux")).toBe("This computer");
    expect(thisComputerText("MacIntel")).toBe("this Mac");
    expect(thisComputerText("Win32")).toBe("this PC");
  });

  it("takes the system the Electron shell reports over the browser's", () => {
    expect(hostOs({ cuaDesktop: { invoke: async () => null, platform: "win32" } })).toBe("windows");
    expect(hostOs({ cuaDesktop: { invoke: async () => null, platform: "linux" } })).toBe("linux");
    expect(hostOs({ cuaDesktop: { invoke: async () => null, platform: "darwin" } })).toBe("macos");
    // Nothing known: the Mac's words.
    expect(osOfPlatform("")).toBe("macos");
    expect(osOfPlatform(undefined)).toBe("macos");
  });

  it("keeps the menu bar and shortcut rows to the Mac", () => {
    expect(showsMacShellSettings("MacIntel")).toBe(true);
    expect(showsMacShellSettings("Win32")).toBe(false);
    expect(showsMacShellSettings("Linux x86_64")).toBe(false);
  });
});
