// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Screen checks for the Settings and Notifications flows (`parity.spec.ts`):
 * each state the replay reached goes on the real screen through
 * `window.__cuaParity.settings`, and the screen must show what the core's
 * view says. Dates the page formats itself (enrolled until, last seen) are
 * checked up to the core's words.
 */

import { expect, type Locator, type Page } from "@playwright/test";

import type { ParityCheckpoint } from "../src/bridge/parity";
import type { SettingsCheckpoint } from "../src/bridge/parity-settings";
import type { SettingsRow } from "../src/bridge/contracts/host";
import { checkAgentKeys } from "./agent-keys";

type Of<K extends SettingsCheckpoint["kind"]> = Extract<SettingsCheckpoint, { kind: K }>;

const of = <K extends SettingsCheckpoint["kind"]>(checkpoints: ParityCheckpoint[], kind: K): Of<K>[] => {
  const seen = new Set<string>();
  return checkpoints.filter((c): c is Of<K> => {
    if (c.kind !== kind) return false;
    const key = JSON.stringify(c);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
};

const texts = (l: Locator) => l.evaluateAll((els) => els.map((el) => (el.textContent ?? "").trim()));
const attrs = (l: Locator, name: string) => l.evaluateAll((els, n) => els.map((el) => el.getAttribute(n) ?? ""), name);

/** Goes to a Settings tab without reloading (the store keeps what the harness put there). */
async function tab(page: Page, label: string): Promise<void> {
  await page.locator('nav[aria-label="Settings"]').getByRole("link", { name: label, exact: true }).click();
}

/* ---- Rows the core lays out ------------------------------------------------ */

interface DrawnRow {
  id: string;
  kind: string;
  enabled: string;
  label: string;
  value: string;
  button: string;
  on: string | null;
  choice: string | null;
}

const readRows = (rows: Locator): Promise<DrawnRow[]> =>
  rows.evaluateAll((els) =>
    els.map((el) => {
      const sw = el.querySelector('[data-slot="switch"]');
      const pressed = el.querySelector('[aria-pressed="true"]');
      const select = el.querySelector('[role="combobox"]');
      const input = el.querySelector('input[data-slot="input"]') as HTMLInputElement | null;
      return {
        id: el.getAttribute("data-setting-row") ?? "",
        kind: el.getAttribute("data-kind") ?? "",
        enabled: el.getAttribute("data-enabled") ?? "",
        label: (el.querySelector("[data-row-label]")?.textContent ?? el.querySelector("button")?.textContent ?? "").trim(),
        value: input ? input.value : (el.querySelector("[data-row-value]")?.textContent ?? "").trim(),
        button: (el.querySelector("[data-row-button]")?.textContent ?? "").trim(),
        on: sw ? String(sw.hasAttribute("data-checked")) : null,
        choice: pressed ? (pressed.textContent ?? "").trim() : select ? (select.textContent ?? "").trim() : null,
      };
    }),
  );

/** What a core row should look like drawn. */
function expectedRow(r: SettingsRow): DrawnRow {
  const active = r.options.find((o) => o.active);
  return {
    id: r.id,
    kind: r.kind,
    enabled: String(r.enabled),
    label: r.label,
    value: r.kind === "field" || r.kind === "secret" || r.kind === "text" || r.kind === "prompt" ? (r.value ?? "") : "",
    button: r.kind === "text" ? (r.button ?? "") : "",
    on: r.kind === "toggle" ? String(r.options.find((o) => o.id === "on")?.active ?? false) : null,
    choice: r.kind === "choice" ? (active?.label ?? null) : null,
  };
}

async function expectRows(rows: Locator, want: SettingsRow[], message: string): Promise<void> {
  const expected = want.map(expectedRow);
  await expect.poll(() => readRows(rows), { message }).toEqual(expected);
}

/* ---- About ----------------------------------------------------------------- */

async function checkAbout(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  const states = of(checkpoints, "about");
  for (const { input, view } of states) {
    await page.evaluate((i) => window.__cuaParity!.settings.showAbout(i), input);
    const step = `About for ${view.versionLine}${view.updates ? "" : ", no updater"}`;
    await expect(page.locator("[data-about-title]"), step).toHaveText(view.title);
    await expect(page.locator("[data-about-version]"), step).toHaveText(view.versionLine);
    await expect.poll(() => texts(page.locator("[data-about-link]")), { message: step }).toEqual(view.links.map((l) => l.label));
    await expect(page.locator("[data-about-copyright]"), step).toHaveText(view.copyright);
    const updates = page.locator("[data-about-updates]");
    if (!view.updates) {
      await expect(updates, step).toHaveCount(0);
      continue;
    }
    const u = view.updates;
    await expect(page.locator("[data-about-check]"), step).toHaveText(u.checkLabel);
    if (u.checkEnabled) await expect(page.locator("[data-about-check]"), step).toBeEnabled();
    else await expect(page.locator("[data-about-check]"), step).toBeDisabled();
    await expect(page.locator("[data-about-last-check]"), step).toHaveText(u.lastCheck);
    await expect(page.locator("[data-about-auto-check]"), step).toHaveAttribute("aria-checked", String(u.autoCheck));
    await expect(page.locator("[data-about-auto-install]"), step).toHaveAttribute("aria-checked", String(u.autoInstall));
    if (u.autoInstallEnabled) await expect(page.locator("[data-about-auto-install]"), step).not.toHaveAttribute("data-disabled");
    else await expect(page.locator("[data-about-auto-install]"), step).toHaveAttribute("data-disabled");
    await expect(updates.getByRole("combobox"), step).toHaveText(u.channels.find((c) => c.active)!.label);
  }
  return states.length;
}

/* ---- Devices --------------------------------------------------------------- */

async function checkDevices(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  let checked = 0;
  let last: Of<"devices"> | undefined;
  for (const cp of checkpoints) {
    if (cp.kind === "devices") {
      last = cp;
      const { input, now, view: v } = cp;
      await page.evaluate(([i, n]) => window.__cuaParity!.settings.showDevices(i, n), [input, now] as const);
      const step = `Devices: ${v.thisDevice.title}`;
      if (v.banner) await expect(page.locator("[data-banner-text]"), step).toHaveText(v.banner.text);
      else await expect(page.locator("[data-devices-banner]"), step).toHaveCount(0);
      await expect(page.locator("[data-this-device-status]"), step).toHaveAttribute("data-this-device-status", v.thisDevice.kind);
      await expect(page.locator("[data-this-device-status]"), step).toContainText(v.thisDevice.title);
      await expect(page.locator("[data-this-device-enroll]"), step).toHaveCount(v.thisDevice.actionLabel ? 1 : 0);
      await expect.poll(() => attrs(page.locator("[data-device-row]"), "data-device-row"), { message: step }).toEqual(v.rows.map((r) => r.id));
      expect(await texts(page.locator("[data-device-title]")), step).toEqual(v.rows.map((r) => r.title));
      const details = await texts(page.locator("[data-device-detail]"));
      v.rows.forEach((r, i) => expect(details[i], `${step}: ${r.id}`).toContain(r.detail));
      for (const r of v.rows) {
        const row = page.locator(`[data-device-row="${r.id}"]`);
        await expect(row.locator("[data-device-approve]"), `${step}: ${r.id} Approve`).toHaveCount(r.actions.includes("approve") ? 1 : 0);
        await expect(row.locator("[data-device-menu]"), `${step}: ${r.id} menu`).toHaveCount(r.actions.some((a) => a === "rename" || a === "revoke") ? 1 : 0);
      }
      expect(await texts(page.locator("[data-recent-text]")), step).toEqual(v.recent.map((a) => a.text));
      expect(await attrs(page.locator("[data-unconfirmed-machine]"), "data-unconfirmed-machine"), step).toEqual(v.unconfirmedMachines.map((m) => m.id));
      checked++;
    } else if (cp.kind === "enroll" && last) {
      const { state, view: v } = cp;
      await page.evaluate(([i, n, s]) => window.__cuaParity!.settings.showDevices(i, n, { enroll: s }), [last.input, last.now, state] as const);
      const sheet = page.locator("[data-enroll]");
      const step = `Enroll sheet, ${state.phase}`;
      await expect(sheet.locator("[data-enroll-title]"), step).toHaveText(v.title);
      expect(await attrs(sheet.locator("[data-enroll-option]"), "data-enroll-option"), step).toEqual(v.options.map((o) => o.method));
      await expect(sheet.locator("[data-enroll-code]"), step).toHaveCount(v.code ? 1 : 0);
      if (v.code) await expect(sheet.locator("[data-enroll-code]"), step).toHaveText(v.code);
      if (v.status) await expect(sheet.locator("[data-enroll-status]"), step).toHaveText(v.status);
      else await expect(sheet.locator("[data-enroll-status]"), step).toHaveCount(0);
      if (v.error) await expect(sheet.locator("[data-enroll-error]"), step).toHaveText(v.error);
      else await expect(sheet.locator("[data-enroll-error]"), step).toHaveCount(0);
      await expect(sheet.locator("[data-enroll-back]"), step).toHaveCount(v.backLabel ? 1 : 0);
      await expect(sheet.locator("[data-enroll-close]"), step).toHaveText(v.closeLabel);
      checked++;
    } else if (cp.kind === "approve" && last) {
      const { state, view: v } = cp;
      await page.evaluate(([i, n, s]) => window.__cuaParity!.settings.showDevices(i, n, { approve: s }), [last.input, last.now, state] as const);
      const sheet = page.locator("[data-approve]");
      const step = `Approval sheet for ${state.name}, code "${state.code}"${state.busy ? ", busy" : ""}`;
      await expect(sheet.locator("[data-approve-title]"), step).toHaveText(v.title);
      await expect(sheet.locator("[data-approve-message]"), step).toHaveText(v.message);
      await expect(sheet.locator("[data-approve-code]"), step).toHaveCount(v.needsCode ? 1 : 0);
      if (v.needsCode) await expect(sheet.locator("[data-approve-code]"), step).toHaveValue(v.code);
      if (v.error) await expect(sheet.locator("[data-approve-error]"), step).toHaveText(v.error);
      else await expect(sheet.locator("[data-approve-error]"), step).toHaveCount(0);
      await expect(sheet.locator("[data-approve-submit]"), step).toHaveText(v.approveLabel);
      if (v.canApprove) await expect(sheet.locator("[data-approve-submit]"), step).toBeEnabled();
      else await expect(sheet.locator("[data-approve-submit]"), step).toBeDisabled();
      await expect(sheet.locator("[data-approve-deny]"), step).toHaveText(v.denyLabel);
      checked++;
    }
  }
  return checked;
}

/* ---- General: launch at login, Storage --------------------------------------- */

async function checkLoginItem(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  const states = of(checkpoints, "login-item");
  for (const { loginItem, experiments, rows } of states) {
    await page.evaluate(([l, x]) => window.__cuaParity!.settings.showLoginItem(l, x), [loginItem, experiments] as const);
    await expectRows(page.locator('[data-setting-row^="launch-at-login"]'), rows, `launch at login, ${JSON.stringify(loginItem)}`);
  }
  return states.length;
}

async function checkStorage(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  const states = of(checkpoints, "storage");
  for (const { input, state, section } of states) {
    await page.evaluate(([i, s]) => window.__cuaParity!.settings.showStorage(i, s), [input, state] as const);
    const drawn = page.locator('[data-settings-section="storage"]');
    const step = `Storage (${input.os}, ${input.storage?.backend ?? "no daemon"}, ${state.request?.kind ?? "idle"})`;
    await expectRows(drawn.locator("[data-setting-row]"), section.rows, step);
    await expect(drawn.locator("[data-section-button]"), step).toHaveCount(section.button ? 1 : 0);
    if (section.button) await expect(drawn.locator("[data-section-button]"), step).toHaveText(section.button);
  }
  return states.length;
}

/* ---- Experiments ------------------------------------------------------------ */

async function checkExperiments(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  let checked = 0;
  for (const { experiments, page: tabPage } of of(checkpoints, "experiments-tab")) {
    await page.evaluate((x) => window.__cuaParity!.settings.showExperiments(x), experiments);
    const rows = tabPage.sections.flatMap((s) => s.rows);
    const toggles = rows.filter((r) => r.kind === "toggle");
    const step = `Experiments tab, ${JSON.stringify(experiments)}`;
    const drawn = page.locator('[data-setting-row][data-kind="toggle"]');
    await expectRows(drawn, toggles, step);
    const notes = await texts(drawn.locator("[data-row-description]"));
    expect(notes, step).toEqual(toggles.map((t) => rows.find((r) => r.id === `${t.id}-note`)?.label ?? ""));
    checked++;
  }
  // Then General: Storage after it only with Cua Volume, and the launch-at-login line.
  await tab(page, "General");
  for (const { experiments, sections } of of(checkpoints, "with-storage")) {
    await page.evaluate((x) => window.__cuaParity!.settings.showExperiments(x), experiments);
    await expect(page.locator('[data-settings-section="storage"]'), `Storage with ${JSON.stringify(experiments)}`).toHaveCount(sections.includes("storage") ? 1 : 0);
    checked++;
  }
  return checked + (await checkLoginItem(page, checkpoints));
}

/* ---- Notifications ----------------------------------------------------------- */

async function checkNotifications(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  const states = of(checkpoints, "notifications");
  for (const { feed, nowMs, view } of states) {
    await page.evaluate(([f, n]) => window.__cuaParity!.settings.showNotifications(f, n), [feed, nowMs] as const);
    const step = `Notifications, ${feed.length} in the feed`;
    await expect.poll(() => attrs(page.locator("[data-notification]"), "data-notification"), { message: step }).toEqual(view.rows.map((r) => r.id));
    expect(await texts(page.locator("[data-notification-text]")), step).toEqual(view.rows.map((r) => r.text));
    expect(await texts(page.locator("[data-notification-time]")), step).toEqual(view.rows.map((r) => r.trailing));
    expect(await attrs(page.locator("[data-notification]"), "data-unread"), step).toEqual(view.rows.map((r) => String(Boolean(r.on))));
    await expect(page.locator("[data-mark-all]"), step).toHaveCount(view.markAllLabel ? 1 : 0);
    if (view.markAllLabel) await expect(page.locator("[data-mark-all]"), step).toHaveText(view.markAllLabel);
    if (view.rows.length === 0) await expect(page.getByText(view.emptyText), step).toBeVisible();
    // The sidebar's badge counts the unread.
    if (view.unread > 0) await expect(page.locator("[data-unread-badge]"), step).toHaveText(String(view.unread));
    else await expect(page.locator("[data-unread-badge]"), step).toHaveCount(0);
  }
  return states.length;
}

/** The screen check for each Settings and Notifications flow. */
export const SETTINGS_CHECKS: Record<string, (page: Page, checkpoints: ParityCheckpoint[]) => Promise<number>> = {
  about: checkAbout,
  "agent-keys": checkAgentKeys,
  devices: checkDevices,
  "drive-storage": checkStorage,
  experiments: checkExperiments,
  "launch-at-login": checkLoginItem,
  notifications: checkNotifications,
};
