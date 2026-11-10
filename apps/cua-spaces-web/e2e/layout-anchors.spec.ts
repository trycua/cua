// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Nothing jumps when you move between first-run steps or Settings tabs, at the window sizes people use.
 *
 * First run (the new setup, Labs): the logo, the headline's top edge, the step dots and the primary button stay put
 * on every step. Steps differ in height; they used to be centred, so the headline and the button moved up and down,
 * and the header and the dots' row changed height with Skip for now and the dots.
 *
 * Settings: the heading, the tab bar and the content keep their left edge on every tab. The app styles its
 * scrollbars, and WebKit (the Mac app's web view) reserves no `scrollbar-gutter` for a styled scrollbar, so a tab
 * that scrolls was 10 px narrower than one that doesn't. Chromium does reserve it; the test turns the gutter off to
 * see the page as WebKit does.
 */

import type { Page } from "@playwright/test";

import { expect, test } from "./host";

const SIZES = [
  [1000, 640],
  [1180, 820],
  [1440, 900],
] as const;

/** 1 px: subpixel rounding, never a layout change. */
const TOLERANCE = 1;

type Box = { x: number; y: number; w: number; h: number };

const box = (page: Page, selector: string) =>
  page.evaluate((sel) => {
    const el = document.querySelector(sel);
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: r.left, y: r.top, w: r.width, h: r.height };
  }, selector);

/** Each anchor's edges on every step agree with the first step that has it, within the tolerance. */
function expectSteady(rows: Record<string, Record<string, Box | null>>, edges: Record<string, (b: Box) => number>, where: string) {
  const steps = Object.keys(rows);
  for (const [anchor, edge] of Object.entries(edges)) {
    const [name, side] = anchor.split(".") as [string, string];
    const seen = steps.flatMap((s) => (rows[s]![name] ? [[s, edge(rows[s]![name]!)] as const] : []));
    expect(seen.length, `${where}: ${name} on no step`).toBeGreaterThan(0);
    const [firstStep, first] = seen[0]!;
    for (const [step, v] of seen) {
      expect(Math.abs(v - first), `${where}: ${name} ${side} on ${step} is ${v.toFixed(1)}, on ${firstStep} ${first.toFixed(1)}`).toBeLessThanOrEqual(
        TOLERANCE,
      );
    }
  }
}

test.use({
  // Headless Chromium hides scrollbars; a Mac with a mouse (or "always show scrollbars") shows them, and they take width.
  launchOptions: { ignoreDefaultArgs: ["--hide-scrollbars"] },
});

for (const [width, height] of SIZES) {
  test.describe(`${width}x${height}`, () => {
    test("first run: the logo, headline, step dots and primary button stay put on every step", async ({ page }) => {
      await page.setViewportSize({ width, height });
      // signin-wait: the browser never confirms, so the sign-in step stays to be measured.
      await page.goto("/onboarding?bridge=demo&demo=fresh,signin-wait&parity");
      // A button under the pointer grows (hover:scale) and would read as a move. Moving the pointer away is not enough on
      // Linux: the corner is over the always-on scrollbar, which swallows the move and leaves the old hover in place.
      await page.addStyleTag({ content: "button, a { transform: none !important; transition: none !important; }" });
      const root = page.locator("[data-new-first-run]");
      const primary = page.locator("[data-onboarding-primary]");

      const rows: Record<string, Record<string, Box | null>> = {};
      const measure = async (step: string, button = "[data-onboarding-primary]") => {
        await expect(root).toHaveAttribute("data-step", step, { timeout: 30_000 });
        // Off the buttons: a hovered button grows (hover:scale) and would read as a move.
        await page.mouse.move(width - 2, height - 2);
        await page.waitForTimeout(300);
        rows[step] = {
          logo: await box(page, "[data-new-run-logo]"),
          title: await box(page, "[data-onboarding-heading]"),
          dots: await box(page, "[data-new-run-dots-row]"),
          primary: await box(page, button),
        };
      };

      await measure("welcome");
      await primary.click();
      await measure("signin", "[data-new-run-try-again]");
      await page.locator("[data-onboarding-later]").click();
      // Skipping sign-in goes straight on: there is no step to pick where agents work (Francesco, 8 Nov 2026).
      // On a Mac host, Cua Spaces' permissions come next (CUA-3550): its button sits in the same pinned row.
      await expect(root).toHaveAttribute("data-step", /^(permissions|agent)$/, { timeout: 30_000 });
      if ((await root.getAttribute("data-step")) === "permissions") {
        await page.locator("[data-new-run-permissions-next]").waitFor();
        await measure("permissions", "[data-new-run-permissions-next]");
        await primary.click();
      }
      await page.locator("[data-new-run='agent']:not([data-state='looking'])").waitFor({ timeout: 30_000 });
      await measure("agent");
      await primary.click();
      // Ready is setup's last screen: Done goes to Home, where the rest is asked for after the first job (CUA-3150).
      await measure("ready");

      expectSteady(
        rows,
        {
          "logo.left": (b) => b.x,
          "logo.top": (b) => b.y,
          "title.left": (b) => b.x,
          "title.top": (b) => b.y,
          "dots.top": (b) => b.y,
          "dots.height": (b) => b.h,
          "primary.left": (b) => b.x,
          "primary.top": (b) => b.y,
        },
        `${width}x${height}`,
      );
    });

    test("Settings: the heading, tab bar and content keep their place on every tab", async ({ page }) => {
      await page.setViewportSize({ width, height });
      await page.goto("/settings?bridge=demo&parity");
      // WebKit reserves no gutter for the app's styled scrollbars: see the page as it does.
      await page.addStyleTag({ content: "* { scrollbar-gutter: auto !important; }" });
      const nav = page.getByRole("tablist", { name: "Settings" });
      await nav.waitFor();

      const rows: Record<string, Record<string, Box | null>> = {};
      for (const tab of ["Account", "General", "Connections", "Advanced", "About"]) {
        await nav.getByRole("tab", { name: tab, exact: true }).click();
        await expect(nav.getByRole("tab", { name: tab, exact: true })).toHaveAttribute("aria-selected", "true");
        await page.waitForTimeout(300);
        rows[tab] = {
          heading: await box(page, "h1"),
          nav: await box(page, 'nav[aria-label="Settings"]'),
          content: await box(page, 'nav[aria-label="Settings"] + *'),
        };
      }

      expectSteady(
        rows,
        {
          "heading.left": (b) => b.x,
          "heading.top": (b) => b.y,
          "nav.left": (b) => b.x,
          "nav.width": (b) => b.w,
          "content.left": (b) => b.x,
          "content.width": (b) => b.w,
        },
        `${width}x${height}`,
      );
    });
  });
}
