// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The bridge inside the built Electron shell (`PARITY_HOST=electron` only),
 * run with its test switch (no native library; the page plays the demo
 * host): the preload exposes `window.cuaDesktop` with the shell's
 * platform, requests on the bridge channel come back in the native hosts'
 * envelope with their id, and the preload refuses every other channel.
 * The answers themselves need the native library: the shell's vitest suite
 * checks them (apps/cua-spaces-desktop/test). Then the Spaces page draws
 * the demo host's Spaces.
 */

import { ELECTRON_BRIDGE_CHANNEL } from "../src/bridge/electron-channels";
import { ELECTRON, expect, test } from "./host";

type Desktop = {
  cuaDesktop?: { invoke(channel: string, args?: unknown): Promise<unknown>; on?: unknown; platform?: string };
};

test.describe("the Electron shell's bridge", () => {
  test.skip(!ELECTRON, "runs on the Electron shell (PARITY_HOST=electron)");

  test("the preload carries bridge requests in the envelope and nothing else", async ({ page }) => {
    await page.goto("/spaces");
    const shape = await page.evaluate(() => {
      const d = (window as Desktop).cuaDesktop;
      return { invoke: typeof d?.invoke, on: typeof d?.on, platform: d?.platform };
    });
    // The parity flows play a Mac on any OS (host.ts).
    expect(shape).toEqual({ invoke: "function", on: "function", platform: "darwin" });
    const invoke = (channel: string, args: unknown) =>
      page.evaluate(([c, a]) => (window as Desktop).cuaDesktop!.invoke(c, a), [channel, args] as const);
    // Under the test switch the shell has no native host: it says so, in the envelope.
    expect(await invoke(ELECTRON_BRIDGE_CHANNEL, { id: "e2e-1", method: "app.info", args: {} })).toMatchObject({
      id: "e2e-1",
      ok: false,
      error: { code: "unsupported" },
    });
    expect(await invoke("cua:spaces.list", {})).toMatchObject({ ok: false, error: { code: "forbidden" } });
  });

  test("the Spaces page draws the demo host's Spaces", async ({ page }) => {
    await page.goto("/spaces");
    await expect(page.locator("[data-space-id]").first()).toBeVisible({ timeout: 20_000 });
    const ids = await page.locator("[data-space-id]").evaluateAll((els) => els.map((e) => e.getAttribute("data-space-id")));
    expect(ids).toContain("local:design-review");
  });
});
