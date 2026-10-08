// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { creatingLine, hostOs, keyvaultUnlockWay, osOfPlatform, showsMacShellSettings, thisComputerLabel, thisComputerText } from "./host-labels";

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

  it("names this computer in a Space being created here", () => {
    const line = "This Mac \u00b7 Downloading image\u2026";
    expect(creatingLine(line, "darwin")).toBe(line);
    expect(creatingLine(line, "linux")).toBe("This computer \u00b7 Downloading image\u2026");
    expect(creatingLine(line, "win32")).toBe("This PC \u00b7 Downloading image\u2026");
    // Elsewhere, or a failure in the core's own words: as it is.
    expect(creatingLine("Cua Cloud \u00b7 Starting\u2026", "linux")).toBe("Cua Cloud \u00b7 Starting\u2026");
    expect(creatingLine("This Mac can't reach its new VM", "linux")).toBe("This Mac can't reach its new VM");
  });

  it("says what keeps the Keyvault's key on each system", () => {
    expect(keyvaultUnlockWay("darwin")).toBe("Touch ID");
    expect(keyvaultUnlockWay("win32")).toBe("Windows Hello");
    expect(keyvaultUnlockWay("linux")).toBe("the system keyring");
  });
});
