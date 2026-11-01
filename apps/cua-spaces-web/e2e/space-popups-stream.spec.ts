// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * CUA-2641: popups, menus, cards, dialogs and tooltips opened from a Space's
 * page never hide, blank or resize its desktop stream.
 *
 * On macOS the SwiftUI host draws the desktop itself over the web view; the
 * page only reports where it goes and what floats over it (apps/cua-spaces-
 * macos/docs/native-video.md). This spec stands in for that host with a
 * `cuaVideo` handler that records every report, runs the real page in a real
 * browser (real hit-testing and layout), opens each popup type with the
 * stream "running", and checks the last report for the viewer: still
 * visible, not occluded, the same size as before, with a hole where the
 * popup is (so the host cuts the video out there and keeps the rest playing),
 * and, under a modal dialog (the dimmed layer), hidden and non-interactive:
 * the native layer cannot sit under the dialog, so the host draws nothing and
 * takes no pointer or keys until it closes (CUA-2758).
 */

import type { Page } from "@playwright/test";

import { ELECTRON, expect, test } from "./host";

// The Electron shell draws video with WebCodecs, not the SwiftUI host's native views this stands in for.
test.skip(ELECTRON, "native video slots are the SwiftUI host's");

interface Surface {
  surfaceId: string;
  tier: "tile" | "full";
  interactive: boolean;
  rect: { x: number; y: number; width: number; height: number };
  clip: { x: number; y: number; width: number; height: number } | null;
  occluded: boolean;
  visible: boolean;
  holes?: { x: number; y: number; width: number; height: number }[];
}

const SPACE = `/spaces/${encodeURIComponent("relay:linux-box/ubuntu-build")}`;

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const w = window as unknown as { __video: unknown[]; webkit: unknown };
    w.__video = [];
    w.webkit = { messageHandlers: { cuaVideo: { postMessage: (m: unknown) => w.__video.push(m) } } };
  });
});

/** The newest report for the viewer (the full tier slot). */
async function viewer(page: Page): Promise<Surface> {
  return page.evaluate(() => {
    const state = new Map<string, Surface>();
    for (const m of (window as unknown as { __video: { type: string; update?: Surface[]; remove?: string[] }[] }).__video) {
      if (m.type !== "surfaces") continue;
      for (const u of m.update ?? []) state.set(u.surfaceId, u);
      for (const id of m.remove ?? []) state.delete(id);
    }
    return [...state.values()].find((s) => s.tier === "full")!;
  });
}

/** Tells the page the host's stream is live, as the host does when frames arrive. */
async function live(page: Page, surfaceId: string) {
  await page.evaluate((id) => window.dispatchEvent(new CustomEvent("cua:event", { detail: { event: "video.surface", payload: { surfaceId: id, state: "live" } } })), surfaceId);
}

const overlaps = (a: Surface["rect"], b: Surface["rect"]) => a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

test("every popup on a Space's page leaves the stream drawn, the same size, with a hole where it floats", async ({ page }) => {
  await page.goto(`${SPACE}?bridge=demo&parity`);
  await expect(page.locator("[data-stream-surface]")).toBeVisible();
  await expect.poll(async () => (await viewer(page))?.visible).toBe(true);
  const before = await viewer(page);
  await live(page, before.surfaceId);
  expect(before.occluded).toBe(false);
  expect(before.holes ?? []).toEqual([]);
  expect(before.interactive).toBe(true);

  /**
   * The popup is open: the stream is still drawn at its size and place, and
   * where the popup floats over the picture there is a hole for it (a popup
   * that floats elsewhere, like a menu that opens over the header, needs none).
   */
  async function expectDrawn(name: string, popup: string, modal: boolean) {
    const el = page.locator(popup).first();
    await expect(el, name).toBeVisible();
    // Reports follow layout over a few frames.
    await page.waitForTimeout(400);
    const box = (await el.boundingBox())!;
    expect(box, name).not.toBeNull();
    const now = await viewer(page);
    const over = overlaps(box, before.rect);
    if (over && !modal) {
      await expect
        .poll(async () => ((await viewer(page)).holes ?? []).some((h) => overlaps(h, box)), { message: `${name}: a hole where the popup is`, timeout: 5_000 })
        .toBe(true);
    }
    const drawn = await viewer(page);
    // A modal dialog hides the video (no hole, nothing drawn over its edge); anything else leaves it drawn.
    await expect.poll(async () => (await viewer(page)).occluded, { message: `${name}: ${modal ? "hidden under a modal dialog" : "not hidden"}`, timeout: 5_000 }).toBe(modal);
    if (modal) expect((await viewer(page)).holes ?? [], `${name}: no hole to leak through`).toEqual([]);
    expect(drawn.visible, `${name}: still visible`).toBe(true);
    expect({ w: drawn.rect.width, h: drawn.rect.height }, `${name}: not resized`).toEqual({ w: before.rect.width, h: before.rect.height });
    expect({ x: drawn.rect.x, y: drawn.rect.y }, `${name}: not moved`).toEqual({ x: before.rect.x, y: before.rect.y });
    expect(drawn.interactive, `${name}: ${modal ? "takes no input under a modal dialog" : "stays interactive"}`).toBe(!modal);
    void now;
    return over;
  }

  /** The popup is closed: no holes, interactive again, same rect. */
  async function expectClear(name: string) {
    await expect.poll(async () => ((await viewer(page)).holes ?? []).length, { message: `${name}: holes gone`, timeout: 5_000 }).toBe(0);
    const now = await viewer(page);
    expect(now.interactive, `${name}: interactive again`).toBe(true);
    expect(now.occluded, name).toBe(false);
    expect(now.rect, `${name}: same rect`).toEqual(before.rect);
  }

  // The profile card (a Popover): click a name in "In this Space".
  const overlapped: string[] = [];
  const track = async (name: string, popup: string, modal: boolean) => {
    if (await expectDrawn(name, popup, modal)) overlapped.push(name);
  };
  await page.locator("[data-profile-trigger='claude']").first().click();
  await track("profile card", "[data-profile-card='agent']", false);
  // A menu opened from inside the card: the rights listbox in its Details.
  await page.locator("[data-card-details]").click();
  const rights = page.locator("[data-profile-card='agent'] [role='combobox'], [data-profile-card='agent'] button[aria-haspopup='listbox']").first();
  if (await rights.count()) {
    await rights.click();
    await track("member row menu", "[role='listbox']", false);
    await page.keyboard.press("Escape");
  }
  await page.keyboard.press("Escape");
  await expect(page.locator("[data-profile-card='agent']")).toHaveCount(0);
  await expectClear("profile card closed");

  // The header's ⋯ menu.
  await page.locator("[data-header-more]").click();
  await track("⋯ menu", "[role='menu']", false);
  await page.keyboard.press("Escape");
  await expectClear("⋯ menu closed");

  // A tooltip (hover the picture-in-picture button).
  await page.locator("[data-pip]").first().hover();
  await track("tooltip", "[data-video-occluder]:not([role='dialog'])", false);
  await page.mouse.move(5, 5);
  await expectClear("tooltip closed");

  // More (the rail's Details sheet): a modal dialog.
  await page.locator("[data-call-more]").click();
  await track("More", "[data-call-more-sheet]", true);
  await page.keyboard.press("Escape");
  await expectClear("More closed");

  // Share: a modal dialog.
  await page.locator("[data-share-space]").first().click();
  await track("Share", "[role='dialog']:not([data-profile-card])", true);
  await page.keyboard.press("Escape");
  await expectClear("Share closed");

  // The check means something only if popups really floated over the picture.
  expect(overlapped, "popups that floated over the picture").toEqual(expect.arrayContaining(["profile card", "More", "Share"]));
});
