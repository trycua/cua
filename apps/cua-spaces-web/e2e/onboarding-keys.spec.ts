// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The first run's keycaps (the agent's key and the Cua key) show at every width the Mac app's
 * window allows (720 px and up). The window keeps its last size, so a first run opened from
 * "Finish setting up" is often narrower than 1000 px; the keys used to drop out there.
 */

import { expect, test } from "./host";

for (const [width, height] of [[720, 560], [980, 660], [1200, 780]] as const) {
  test(`a ${width} px window shows the keys`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await page.goto("/onboarding?bridge=demo&demo=fresh&parity");
    await page.locator("[data-onboarding-heading]").waitFor();
    const art = page.locator("[data-first-run-art]");
    await expect(art).toBeVisible();
    const keys = art.locator("img");
    await expect(keys).toHaveCount(2);
    for (const key of await keys.all()) {
      await expect(key).toBeVisible();
      // Visible is not loaded: the image can still be fetching when it lays out, so wait for its
      // pixels. A broken image fails the visible check above and never gets a width here.
      await expect.poll(() => key.evaluate((i: HTMLImageElement) => i.complete && i.naturalWidth > 0)).toBe(true);
    }
    // The keys sit beside the words, never over them.
    const words = await page.locator("[data-onboarding-heading]").boundingBox();
    const box = await art.boundingBox();
    expect(words!.x + words!.width).toBeLessThanOrEqual(box!.x + 1);
  });
}
