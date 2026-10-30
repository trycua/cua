// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * New Space's arrow: "Add a machine by its address" opens the address form
 * (it does not use the image picker), and Run on has no such entry.
 */

import { expect, test } from "./host";

test("New Space's arrow adds a machine by its address", async ({ page }) => {
  await page.goto("/spaces?bridge=demo&parity");
  const more = page.locator("[data-new-space-more]");
  await more.waitFor();
  await more.click();
  const item = page.getByRole("menuitem", { name: "Add a machine by its address…" });
  await expect(item).toBeVisible();
  await item.click();
  const wizard = page.locator("[data-new-space]");
  await expect(wizard).toHaveAttribute("data-mode", "address");
  await expect(wizard.locator("[data-image-input]")).toHaveCount(0);
  await expect(wizard.locator("[data-run-on]")).toHaveCount(0);
  // Back returns to the wizard's steps.
  await wizard.locator("[data-wizard-back]").click();
  await expect(wizard).toHaveAttribute("data-mode", "create");
  await wizard.locator("[data-run-on]").click();
  const values = await page.locator("[data-run-on-option]:visible").evaluateAll((els) => els.map((el) => el.getAttribute("data-value") ?? ""));
  expect(values.some((v) => v.includes("address"))).toBe(false);
});
