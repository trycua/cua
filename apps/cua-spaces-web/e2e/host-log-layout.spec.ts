// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * This machine's logs at a real window width (1100 px, as on a Mac): a long row gives way in the middle, so what happened ("Screen and
 * input refused (desktop not shared) ×12") stays whole, on the page and in
 * Show All; the Spaces activity keeps its action and Space whole instead.
 */

import type { Locator } from "@playwright/test";

import { expect, test } from "./host";

const WHO = "Ada Lovelace (5a1c7e02-3b4d-4e6f-8a9b-0c1d2e3f4a5b)";
const now = Date.now();
const refused = (i: number) => ({ atMs: now - i * 1000, via: "relay", who: WHO, what: "refused ComputerService/Stream (desktop not shared)" });
const spaces = (i: number) => ({ atMs: now - 100_000 - i * 1000, via: "relay", who: WHO, what: "HostSpacesService/List" });
// Twelve refusals in a row (collapsed to one "×12" row), then a run of others.
const recentAccess = [...Array.from({ length: 12 }, (_, i) => refused(i)), ...Array.from({ length: 6 }, (_, i) => [spaces(i), refused(20 + i)]).flat()];
const spacesAudit = Array.from({ length: 7 }, (_, i) => ({ atMs: now - i * 60_000, action: "delete", who: WHO, space: `space-e0${i}a1b2c3d4e5f6`, detail: "" }));

const state = {
  configured: true, mode: "relay", relayUrl: "https://relay.cua.ai", directUrl: null, name: "Studio", sharing: true,
  serviceInstalled: true, serviceRunning: true, serviceKind: "launchd", online: true, clients: [], permissions: [], error: null,
  recentAccess, accessLogError: null, shareDesktop: false, provideSpaces: true, maxSpaces: 4, maxMacosVms: 2,
  providedSpaces: [], spacesAudit, spacesAuditError: null, pausedSignedOut: false, owner: null, ownerEmail: null, account: null,
};

/** The part of a row that must stay whole is not cut, and fits in its row. */
async function whole(part: Locator): Promise<void> {
  const fit = await part.evaluate((el) => {
    const row = el.closest("[data-host-log-row]")!.getBoundingClientRect();
    const r = el.getBoundingClientRect();
    return { cut: el.scrollWidth > el.clientWidth + 1, inside: r.right <= row.right + 0.5 && r.left >= row.left - 0.5 };
  });
  expect(fit).toEqual({ cut: false, inside: true });
}

test.use({ viewport: { width: 1100, height: 800 } });

/** Fonts wider than macOS' (Linux runners' DejaVu, spaced out): the kept
 * part still never truncates; it wraps when it alone is wider than the row. */
const WIDE_FONT = '* { font-family: "DejaVu Sans", Verdana, sans-serif !important; letter-spacing: .5px !important; }';

for (const [name, css] of [
  ["the system font", null],
  ["a wider font", WIDE_FONT],
] as const) {
  test(`This machine's long log rows keep what happened in view at 1100 px, in ${name}`, async ({ page }) => {
    await page.goto("/machines?bridge=demo&parity");
    await page.waitForFunction(() => window.__cuaParity !== undefined);
    if (css) await page.addStyleTag({ content: css });
    await page.locator("[data-host-panel]").waitFor();
    await page.evaluate((s) => window.__cuaParity!.showHost(s), state);

    const tails = page.locator("[data-host-log-row] [data-host-log-tail]");
    await expect(tails.first()).toHaveText("Screen and input refused (desktop not shared) ×12");
    for (const tail of await tails.all()) {
      if ((await tail.textContent())?.startsWith("Screen")) await whole(tail);
    }
    // The activity: "Deleted space-…" stays whole; who gives way.
    const activityHead = page.locator('[data-host-log-row]:has-text("Deleted") [data-host-log-head]').first();
    await expect(activityHead).toHaveText(/^Deleted space-e00a1b2c3d4e5f6 · $/);
    await whole(activityHead);

    // Show All: the same in the sheet.
    await page.locator('[data-host-more="access"]').click();
    const sheet = page.locator("[data-host-log]");
    await expect(sheet).toBeVisible();
    const sheetTail = sheet.locator("[data-host-log-tail]").first();
    await expect(sheetTail).toHaveText("Screen and input refused (desktop not shared) ×12");
    await whole(sheetTail);
  });
}
