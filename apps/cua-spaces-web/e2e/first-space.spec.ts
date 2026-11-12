// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Your computers with no Space yet and New Space on the SwiftUI host's answer
 * (`?demo=mac-host`: `spaces.createOptions` in `WebUIBridge.createOptions`'s
 * shape, its own wizard env, two macOS VMs of Lume's running), loaded while
 * the launch is still at the Keychain: once it is ready, New Space on This
 * Mac opens on the defaults (its Resources step says what is free on
 * Macintosh HD), macOS says Apple's limit, and nothing is created by itself.
 */

import { expect, test } from "./host";

test("your computers with no Space yet, and New Space on the macOS host's answer, loaded before the host is ready", async ({ page }) => {
  await page.goto("/spaces?bridge=demo&demo=empty,keychain,mac-host");
  await expect(page.getByTestId("startup-screen")).toBeVisible();
  await page.getByRole("button", { name: "Allow access" }).click();
  await expect(page.getByTestId("startup-screen")).toHaveCount(0, { timeout: 10_000 });

  // Your computers, with no Space on them yet; New Space is in the header.
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Computers");
  await page.locator("[data-spaces-tree]").waitFor();
  await expect(page.locator('[data-tree-kind="space"]')).toHaveCount(0);

  // New Space: the defaults (Linux on This Mac), then Resources.
  await page.getByRole("button", { name: "New Space", exact: true }).click();
  const wizard = page.locator("[data-new-space]");
  await expect(wizard).toHaveAttribute("data-step", "0");
  await expect(wizard.locator('[data-tile="os:linux"]')).toHaveAttribute("aria-pressed", "true");
  await wizard.locator("[data-wizard-primary]").click();
  await expect(wizard).toHaveAttribute("data-step", "1");
  await expect(wizard.locator('[data-fact="available"] [data-fact-value]')).toHaveText("212 GB on Macintosh HD");
  await wizard.locator("[data-wizard-back]").click();

  // macOS on This Mac stops before the download: it says why, and nothing is created.
  await wizard.locator('[data-tile="os:macos"]').click();
  await expect(wizard.getByRole("alert").filter({ hasText: "2 macOS virtual machines" })).toBeVisible();
  await expect(wizard.locator("[data-wizard-primary]")).toBeDisabled();
  await wizard.locator("[data-wizard-cancel]").click();
  await expect(wizard).toHaveCount(0);
  await expect(page.locator('[data-tree-kind="space"]')).toHaveCount(0);
});

test("New Space stops a macOS create at Apple's limit when the Mac's two macOS VMs aren't Spaces", async ({ page }) => {
  // Spaces in the list (none of them a macOS Space on this Mac), two macOS VMs of Lume's running.
  await page.goto("/spaces?bridge=demo&demo=mac-host");
  await page.locator("[data-spaces-tree]").waitFor();
  await page.getByRole("button", { name: "New Space" }).first().click();
  const wizard = page.locator("[data-new-space]");
  await expect(wizard).toHaveAttribute("data-step", "0");
  await wizard.locator('[data-tile="os:macos"]').click();
  await expect(wizard.getByRole("alert").filter({ hasText: "This Mac is already running 2 macOS virtual machines" })).toHaveText(
    "This Mac is already running 2 macOS virtual machines, the most Apple's macOS license allows at once. Stop one, then create this one.",
  );
  await expect(wizard.locator("[data-wizard-primary]")).toBeDisabled();
  // Linux is not limited.
  await wizard.locator('[data-tile="os:linux"]').click();
  await expect(wizard.getByRole("alert").filter({ hasText: "macOS virtual machines" })).toHaveCount(0);
  await expect(wizard.locator("[data-wizard-primary]")).toBeEnabled();
});
