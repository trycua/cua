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
  it("tells Windows users to start a stopped Docker Desktop", () => {
    expect(noLocalRuntimeReason("win32", "container engine at npipe://x is not running")).toMatch(/isn't running/);
  });
});
