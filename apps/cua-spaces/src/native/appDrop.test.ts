// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it, vi } from "vitest";

import { handleAppDrop, hasAppPath } from "./appDrop";
import { createFallbackTeleportAppsBridge, FIXTURE_CATALOG } from "./teleportApps";

// Fixture catalog and fixture paths only: nothing reads this machine's apps.
const apps = createFallbackTeleportAppsBridge();
const space = { id: "cloud:aurora", name: "Aurora" };

describe("app drops on a Space", () => {
  it("opens the picker preselected with a dropped app and its files", async () => {
    const open = vi.fn();
    const handled = await handleAppDrop(
      ["/Applications/Visual Studio Code.app/", "/tmp/fixture-project"],
      space,
      open,
      apps,
    );
    expect(handled).toBe(true);
    expect(open).toHaveBeenCalledWith(
      "cloud:aurora",
      "Aurora",
      { id: "vscode", name: "Visual Studio Code" },
      JSON.parse(FIXTURE_CATALOG[1]!.json),
      ["/tmp/fixture-project"],
    );
  });

  it("leaves file and folder drops to the file transfer", async () => {
    const open = vi.fn();
    expect(hasAppPath(["/tmp/a.txt", "/tmp/dir"])).toBe(false);
    expect(await handleAppDrop(["/tmp/a.txt"], space, open, apps)).toBe(false);
    expect(open).not.toHaveBeenCalled();
  });

  it("still opens the full catalog for an app it cannot read", async () => {
    const open = vi.fn();
    expect(await handleAppDrop(["/Applications/Unknown Thing.app"], space, open, apps)).toBe(true);
    expect(open).toHaveBeenCalledWith("cloud:aurora", "Aurora", null, null, []);
  });
});
