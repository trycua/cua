// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { noLocalRuntimeReason } from "./local-runtime";

describe("noLocalRuntimeReason", () => {
  it("keeps the Mac wording", () => {
    expect(noLocalRuntimeReason("darwin")).toContain("Lume");
  });
  it("never mentions Lume on Windows or Linux", () => {
    for (const p of ["win32", "linux"]) {
      expect(noLocalRuntimeReason(p)).not.toContain("Lume");
      expect(noLocalRuntimeReason(p, "container engine at x is not running")).not.toContain("Lume");
    }
  });
  it("points Windows users at WSL 2, not Docker, when there is no engine", () => {
    const r = noLocalRuntimeReason("win32", "no Docker-API socket found");
    expect(r).toContain("WSL 2");
    expect(r).not.toMatch(/Install it from|Docker Desktop/);
  });
  it("never tells a Windows user to start or install Docker Desktop, whatever the engine says", () => {
    for (const detail of [
      "container engine: Socket not found: //./pipe/dockerDesktopLinuxEngine",
      "container engine at npipe://x is not running",
      "no Docker-API socket found",
    ]) {
      expect(noLocalRuntimeReason("win32", detail)).not.toMatch(/Docker Desktop|Install it from|Start it/);
    }
  });

  it("on Windows says what the built-in runtime needs when WSL 2 cannot run it, never to install Docker", () => {
    const why = "the built-in Linux runtime cannot run here: Virtualization is turned off on this PC. Restart into the BIOS/UEFI setup";
    expect(noLocalRuntimeReason("win32", "container engine: Socket not found: //./pipe/dockerDesktopLinuxEngine", why)).toBe(why);
    expect(noLocalRuntimeReason("win32", "no Docker-API socket found", "some other detail")).not.toMatch(/Docker Desktop/);
  });

  it("names the group command when Docker is there but the account may not use it (CUA-1763)", () => {
    expect(noLocalRuntimeReason("linux", "permission denied on unix:///var/run/docker.sock: add yourself")).toContain("usermod -aG docker");
  });
});
