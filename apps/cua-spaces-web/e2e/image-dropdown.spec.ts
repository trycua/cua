// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * New Space, step 1: the Image list floats over the rows under its field and
 * is not cut off by the System card or the dialog. Every image the core
 * offers for the chosen system is drawn, and each one is reachable (the list
 * scrolls when it is long).
 */

import { expect, test } from "./host";

test("the image list is not clipped by the System card", async ({ page }) => {
  await page.goto("/spaces?bridge=demo&parity");
  await page.getByRole("button", { name: /New Space/ }).first().click();
  const wizard = page.locator("[data-new-space]");
  await expect(wizard).toHaveAttribute("data-step", "0");
  await wizard.locator("[data-image-toggle]").click();
  const list = wizard.locator("[data-image-suggestions]");
  await expect(list).toBeVisible();
  const rows = list.locator("[data-image-row]");
  const count = await rows.count();
  expect(count).toBeGreaterThan(1);
  const card = wizard.locator("[data-grouped]").first();
  const cardBox = (await card.boundingBox())!;
  const listBox = (await list.boundingBox())!;
  // The list is drawn past the card's bottom edge instead of ending there.
  expect(listBox.y + listBox.height).toBeGreaterThan(cardBox.y + cardBox.height);
  // Every row is the topmost thing at its centre once scrolled to.
  for (let i = 0; i < count; i++) {
    const row = rows.nth(i);
    await row.scrollIntoViewIfNeeded();
    const box = (await row.boundingBox())!;
    const hit = await row.evaluate((el, p) => el.contains(document.elementFromPoint(p.x, p.y)), { x: box.x + box.width / 2, y: box.y + box.height / 2 });
    expect(hit, `row ${i} is drawn and not covered or clipped`).toBe(true);
  }
});
