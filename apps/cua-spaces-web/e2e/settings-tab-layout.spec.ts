// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings tabs keep their place: some are long (Connected apps: the page
 * scrolls), the others are short (it doesn't). The page's scrollbar must not take width
 * from the column on one tab and not the other, or the tab bar and content
 * jump sideways when you switch.
 */

import { expect, test } from "./host";

const TABS = [
  ["Account", "/settings"],
  ["General", "/settings/general"],
  ["Connected apps", "/settings/connected-apps"],
  ["Trusted devices", "/settings/devices"],
  ["Advanced", "/settings/advanced"],
  ["About", "/settings/about"],
] as const;

// A window that fits the short tabs but not Connected apps; a Settings tab bar and heading to measure.
test.use({
  viewport: { width: 1100, height: 900 },
  // Headless Chromium hides scrollbars by default; a Mac window with a mouse
  // (or "always show scrollbars") shows them, and they take width.
  launchOptions: { ignoreDefaultArgs: ["--hide-scrollbars"] },
});

test("Settings: the heading, tab bar and content keep their left edge across tabs", async ({ page }) => {
  await page.goto("/settings?bridge=demo&parity");
  // The window's own size is the host's (the Electron window ignores the viewport): fix the page's height here.
  await page.addStyleTag({ content: "html, body, #root { height: 760px !important; max-height: 760px !important; overflow: hidden !important; }" });
  const nav = page.getByRole("tablist", { name: "Settings" });
  await nav.waitFor();

  type Edges = { heading: number; nav: number; content: number; scrolls: boolean };
  const edges: Record<string, Edges> = {};
  for (const [label] of TABS) {
    await nav.getByRole("tab", { name: label, exact: true }).click();
    await expect(nav.getByRole("tab", { name: label, exact: true })).toHaveAttribute("aria-selected", "true");
    await page.waitForTimeout(150);
    edges[label] = await page.evaluate(() => {
      const nav = document.querySelector('nav[aria-label="Settings"]')!;
      const scroller = nav.closest(".overflow-y-auto")!;
      const first = nav.nextElementSibling;
      return {
        heading: document.querySelector("h1")!.getBoundingClientRect().left,
        nav: nav.getBoundingClientRect().left,
        content: first ? first.getBoundingClientRect().left : -1,
        scrolls: scroller.scrollHeight > scroller.clientHeight,
      };
    });
  }

  // The test only means something if the tabs differ in length.
  const scrolling = Object.values(edges).map((e) => e.scrolls);
  expect(scrolling).toContain(true);
  expect(scrolling).toContain(false);

  const base = edges.Account!;
  for (const [label] of TABS) {
    const e = edges[label]!;
    expect({ heading: e.heading, nav: e.nav }, label).toEqual({ heading: base.heading, nav: base.nav });
    // A tab with nothing to show yet has no content to measure.
    if (e.content >= 0) expect(e.content, label).toBe(base.content);
  }
});
