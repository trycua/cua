// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The empty home (under the Spaces header) and New Space on the SwiftUI host's answer
 * (`?demo=mac-host`: `spaces.createOptions` in `WebUIBridge.createOptions`'s
 * shape, its own wizard env, two macOS VMs of Lume's running), loaded while
 * the launch is still at the Keychain: once it is ready, the empty home
 * offers Linux and macOS with the core's time and size; each tile opens New
 * Space with its system chosen (macOS says Apple's limit), New Space opens
 * on the defaults (its Resources step says what is free on Macintosh HD),
 * and no tile creates a Space by itself.
 */

import { expect, test } from "./host";

test("empty home and New Space on the macOS host's answer, loaded before the host is ready", async ({ page }) => {
  await page.goto("/spaces?bridge=demo&demo=empty,keychain,mac-host");
  await expect(page.getByTestId("startup-screen")).toBeVisible();
  await page.getByRole("button", { name: "Allow access" }).click();
  await expect(page.getByTestId("startup-screen")).toHaveCount(0, { timeout: 10_000 });

  const home = page.locator("[data-empty-home]");
  // Under the Spaces header, as with Spaces.
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Spaces");
  await expect(page.getByText("0 Spaces on")).toBeVisible();
  await expect(page.getByLabel("Filter by state").getByRole("button", { name: "Running" })).toBeVisible();
  await expect(home.getByRole("heading")).toHaveText("No Spaces yet");
  await expect(home).toContainText("Choose a system for your first Space.");
  const linux = home.getByRole("button", { name: "Linux, About 1 minute · 3.2–7.1 GB of disk" });
  const macos = home.getByRole("button", { name: "macOS, About 22 GB download" });
  await expect(linux).toBeVisible();
  await expect(macos).toBeVisible();

  // macOS on This Mac stops before the download: New Space opens on it and says why.
  await macos.click();
  const wizard = page.locator("[data-new-space]");
  await expect(wizard).toHaveAttribute("data-step", "0");
  await expect(wizard.locator('[data-tile="os:macos"]')).toHaveAttribute("aria-pressed", "true");
  await expect(wizard.getByRole("alert").filter({ hasText: "2 macOS virtual machines" })).toBeVisible();
  await expect(wizard.locator("[data-wizard-primary]")).toBeDisabled();
  await wizard.locator("[data-wizard-cancel]").click();
  await expect(wizard).toHaveCount(0);

  // The header's New Space (the empty home has none of its own): the
  // defaults (Linux on This Mac), then Resources.
  await expect(home.getByRole("button", { name: "New Space" })).toHaveCount(0);
  await page.getByRole("button", { name: "New Space" }).click();
  await expect(wizard).toHaveAttribute("data-step", "0");
  await expect(wizard.locator('[data-tile="os:linux"]')).toHaveAttribute("aria-pressed", "true");
  await wizard.locator("[data-wizard-primary]").click();
  await expect(wizard).toHaveAttribute("data-step", "1");
  await expect(wizard.locator('[data-fact="available"] [data-fact-value]')).toHaveText("212 GB on Macintosh HD");
  await wizard.locator("[data-wizard-cancel]").click();

  // Linux: New Space with Linux chosen; nothing is created until Create.
  await linux.click();
  await expect(wizard).toHaveAttribute("data-step", "0");
  await expect(wizard.locator('[data-tile="os:linux"]')).toHaveAttribute("aria-pressed", "true");
  await wizard.locator("[data-wizard-cancel]").click();
  await expect(page.locator("[data-space-id]")).toHaveCount(0);
  await expect(home).toBeVisible();
});

test("New Space stops a macOS create at Apple's limit when the Mac's two macOS VMs aren't Spaces", async ({ page }) => {
  // Spaces in the list (none of them a macOS Space on this Mac), two macOS VMs of Lume's running.
  await page.goto("/spaces?bridge=demo&demo=mac-host");
  await page.locator("[data-space-id]").first().waitFor();
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
