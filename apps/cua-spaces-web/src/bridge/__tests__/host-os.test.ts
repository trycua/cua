// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { hostOs, osOfPlatform, THIS_MACHINE } from "../host-os";
import type { HostWindow } from "../detect";

describe("host os: this computer is named for the system it runs on", () => {
  it("maps each platform word to its system", () => {
    expect(osOfPlatform("darwin")).toBe("macos");
    expect(osOfPlatform("MacIntel")).toBe("macos");
    expect(osOfPlatform("win32")).toBe("windows");
    expect(osOfPlatform("Win32")).toBe("windows");
    expect(osOfPlatform("linux")).toBe("linux");
    expect(osOfPlatform("Linux x86_64")).toBe("linux");
  });

  it("reads the system the Electron shell reports", () => {
    const of = (platform: string) => hostOs({ cuaDesktop: { platform } } as unknown as HostWindow);
    expect(THIS_MACHINE[of("darwin")]).toBe("This Mac");
    expect(THIS_MACHINE[of("win32")]).toBe("This PC");
    expect(THIS_MACHINE[of("linux")]).toBe("This computer");
  });
});
