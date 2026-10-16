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
  it("tells Windows users to install Docker Desktop when there is no engine (WSL 2 only)", () => {
    const r = noLocalRuntimeReason("win32", "no Docker-API socket found");
    expect(r).toContain("Install it from https://docs.docker.com/desktop/setup/install/windows-install/");
    expect(r).toContain("WSL 2");
  });
  it("knows Docker Desktop is installed when its pipe is missing", () => {
    expect(noLocalRuntimeReason("win32", "container engine: Socket not found: //./pipe/dockerDesktopLinuxEngine")).toMatch(/isn't running/);
  });
  it("tells Windows users to start a stopped Docker Desktop", () => {
    expect(noLocalRuntimeReason("win32", "container engine at npipe://x is not running")).toMatch(/isn't running/);
  });

  it("names the group command when Docker is there but the account may not use it (CUA-1763)", () => {
    expect(noLocalRuntimeReason("linux", "permission denied on unix:///var/run/docker.sock: add yourself")).toContain("usermod -aG docker");
  });
});
