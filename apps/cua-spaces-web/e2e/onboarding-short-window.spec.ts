// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Every first-run page keeps its action row inside the window, however short the window is: the
 * row sticks to the bottom of the content area and the page scrolls above it. Back and Skip for now
 * stay where they are. The same page height is set the way the Electron shell sets it (the window's
 * own size is the host's, so the page's height is fixed here).
 */

import { expect, test } from "./host";

const PAGES: { name: string; need: string[]; go: (page: import("@playwright/test").Page) => Promise<void> }[] = [
  { name: "Welcome", need: ["Get started", "Skip for now"], go: async () => {} },
  {
    name: "Pick your agent",
    need: ["Set up", "Not now", "Skip for now"],
    go: async (page) => {
      await page.locator("[data-onboarding-primary]").click();
      await page.locator("[data-first-run-set-up]").waitFor({ timeout: 30_000 });
      await page.waitForTimeout(800);
    },
  },
  {
    name: "Command line",
    need: ["Install", "Not now", "Skip for now"],
    go: async (page) => {
      await page.locator("[data-onboarding-primary]").click();
      await page.getByRole("button", { name: "Set up" }).waitFor({ timeout: 30_000 });
      await page.getByRole("button", { name: "Not now" }).click();
      await page.locator("[data-first-run='cli']").waitFor({ timeout: 30_000 });
    },
  },
  {
    name: "Add your other computers",
    need: ["Sign in", "Not now", "Skip for now"],
    go: async (page) => {
      await page.locator("[data-onboarding-primary]").click();
      await page.getByRole("button", { name: "Set up" }).waitFor({ timeout: 30_000 });
      await page.getByRole("button", { name: "Not now" }).click();
      await page.locator("[data-first-run='cli']").waitFor({ timeout: 30_000 });
      await page.getByRole("button", { name: "Not now" }).click();
      await page.locator("[data-first-run='devices']").waitFor({ timeout: 30_000 });
    },
  },
];

for (const height of [600, 700]) {
  test.describe(`a ${height} px window`, () => {
    test.use({ viewport: { width: 1280, height } });
    for (const p of PAGES) {
      test(`${p.name}: every action is inside the window`, async ({ page }) => {
        await page.goto("/onboarding?bridge=demo&demo=fresh,child-spaces,cli-missing&parity");
        await page.addStyleTag({ content: `html, body, #root { height: ${height}px !important; max-height: ${height}px !important; overflow: hidden !important; }` });
        await page.locator("[data-onboarding-heading]").waitFor();
        await p.go(page);
        const inView = await page.evaluate((h) => {
          return [...document.querySelectorAll("button")]
            .filter((b) => {
              const r = b.getBoundingClientRect();
              return r.width > 0 && r.height > 0 && r.top >= 0 && r.bottom <= h;
            })
            .map((b) => (b.textContent ?? "").trim());
        }, height);
        for (const name of p.need) expect(inView, `${name} on ${p.name} at ${height} px (in view: ${inView.join(" | ")})`).toContain(name);
      });
    }
  });
}
