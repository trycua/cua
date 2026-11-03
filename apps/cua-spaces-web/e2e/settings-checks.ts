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
import { LAUNCH_HELP, LAUNCH_LABEL, lastChecked, plainStorage } from "../src/lib/settings-copy";
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

const navigate = (page: Page, path: string) => page.evaluate((p) => window.__cuaParity!.navigate(p), path);

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

/** What a core row should look like drawn. `words` is the page's plain-language pass over the core's text (none: as is). */
function expectedRow(r: SettingsRow, words: (text: string) => string = (t) => t): DrawnRow {
  const active = r.options.find((o) => o.active);
  return {
    id: r.id,
    kind: r.kind,
    enabled: String(r.enabled),
    label: words(r.label),
    value: r.kind === "field" || r.kind === "secret" || r.kind === "text" || r.kind === "prompt" ? words(r.value ?? "") : "",
    button: r.kind === "text" ? words(r.button ?? "") : "",
    on: r.kind === "toggle" ? String(r.options.find((o) => o.id === "on")?.active ?? false) : null,
    choice: r.kind === "choice" ? (active ? words(active.label) : null) : null,
  };
}

async function expectRows(rows: Locator, want: SettingsRow[], message: string, words?: (text: string) => string): Promise<void> {
  const expected = want.map((r) => expectedRow(r, words));
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
    await expect.poll(() => texts(page.locator("[data-about-link-label]")), { message: step }).toEqual(view.links.map((l) => l.label));
    await expect(page.locator("[data-about-copyright]"), step).toHaveText(view.copyright);
    const updates = page.locator("[data-about-updates]");
    if (!view.updates) {
      await expect(updates, step).toHaveCount(0);
      continue;
    }
    const u = view.updates;
    // The page says "Check now", and one switch for checking and installing; Beta is in Advanced.
    await expect(page.locator("[data-about-check]"), step).toHaveText("Check now");
    if (u.checkEnabled) await expect(page.locator("[data-about-check]"), step).toBeEnabled();
    else await expect(page.locator("[data-about-check]"), step).toBeDisabled();
    await expect(page.locator("[data-about-last-check]"), step).toHaveText(lastChecked(u.lastCheck));
    await expect(page.locator("[data-about-auto-check]"), step).toHaveAttribute("aria-checked", String(u.autoCheck));
    await expect(updates.getByRole("combobox"), step).toHaveCount(0);
  }
  return states.length;
}

/* ---- Machines: this machine's trust, the machines waiting, the enroll sheet ---------- */

async function checkDevices(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  let checked = 0;
  let last: Of<"devices"> | undefined;
  for (const cp of checkpoints) {
    if (cp.kind === "devices") {
      last = cp;
      const { input, now, view: v } = cp;
      await page.evaluate(([i, n]) => window.__cuaParity!.settings.showDevices(i, n), [input, now] as const);
      const step = `Machines: ${v.thisDevice.title}`;
      // This machine's page says what the core says under Details: its trust, and what to do about it.
      const toggle = page.locator("[data-machine-details-toggle]");
      if ((await toggle.getAttribute("aria-expanded")) !== "true") await toggle.click();
      await expect(page.locator("[data-machine-trust]"), step).toHaveAttribute("data-machine-trust", v.thisDevice.kind);
      await expect(page.locator("[data-machine-trust-status]"), step).toContainText(v.thisDevice.title);
      await expect(page.locator("[data-machine-trust-action]"), step).toHaveCount(v.thisDevice.actionLabel ? 1 : 0);
      if (v.banner) await expect(page.locator("[data-machine-trust-text]"), step).toHaveText(v.banner.text);
      else await expect(page.locator("[data-machine-trust-text]"), step).toHaveCount(0);
      // The machines waiting to be added are the top group.
      const waiting = [...v.unconfirmedMachines.map((m) => m.id), ...v.approvals.map((a) => a.deviceId)];
      await expect.poll(() => attrs(page.locator("[data-pending-machine]"), "data-pending-machine"), { message: step }).toEqual(waiting);
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
    }
  }
  return checked;
}

/* ---- General: launch at login, Storage --------------------------------------- */

async function checkLoginItem(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  const states = of(checkpoints, "login-item");
  for (const { loginItem, experiments, rows } of states) {
    await page.evaluate(([l, x]) => window.__cuaParity!.settings.showLoginItem(l, x), [loginItem, experiments] as const);
    // The page words the switch itself and puts its one line (the core's note) under the label.
    const shown = rows.filter((r) => r.id !== "launch-at-login-note").map((r) => (r.id === "launch-at-login" ? { ...r, label: LAUNCH_LABEL } : r));
    await expectRows(page.locator('[data-setting-row^="launch-at-login"]'), shown, `launch at login, ${JSON.stringify(loginItem)}`);
    await expect(page.locator('[data-setting-row="launch-at-login"] [data-row-description]'), `launch at login, ${JSON.stringify(loginItem)}`).toHaveText(LAUNCH_HELP);
  }
  return states.length;
}

async function checkStorage(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  const states = of(checkpoints, "storage");
  for (const { input, state, section } of states) {
    await page.evaluate(([i, s]) => window.__cuaParity!.settings.showStorage(i, s), [input, state] as const);
    const drawn = page.locator('[data-settings-section="storage"]');
    const step = `Storage (${input.os}, ${input.storage?.backend ?? "no daemon"}, ${state.request?.kind ?? "idle"})`;
    await expectRows(drawn.locator("[data-setting-row]"), section.rows, step, plainStorage);
    await expect(drawn.locator("[data-section-button]"), step).toHaveCount(section.button ? 1 : 0);
    if (section.button) await expect(drawn.locator("[data-section-button]"), step).toHaveText(section.button);
  }
  return states.length;
}

/* ---- Experiments ------------------------------------------------------------ */

/** Advanced keeps Labs: Cua Volume. Sharing, Your cloud and the New UI have no switch. */
const LABS: Record<string, { label?: string; note: string }> = {
  "experiment:cua_volume": { note: "Memory and files your agents share across every Space." },
};

async function checkExperiments(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  let checked = 0;
  for (const { experiments, page: tabPage } of of(checkpoints, "experiments-tab")) {
    await page.evaluate((x) => window.__cuaParity!.settings.showExperiments(x), experiments);
    const toggles = tabPage.sections.flatMap((s) => s.rows).filter((r) => r.kind === "toggle" && r.id in LABS);
    const step = `Advanced, Labs, ${JSON.stringify(experiments)}`;
    const drawn = page.locator('[data-settings-tab="advanced"] [data-setting-row^="experiment:"]');
    await expectRows(drawn, toggles.map((t) => ({ ...t, label: LABS[t.id]!.label ?? t.label })), step);
    expect(await texts(drawn.locator("[data-row-description]")), step).toEqual(toggles.map((t) => LABS[t.id]!.note));
    checked++;
  }
  // Then the Volume page: Storage after it only with Cua Volume, and General's launch-at-login line.
  await navigate(page, "/volume");
  for (const { experiments, sections } of of(checkpoints, "with-storage")) {
    await page.evaluate((x) => window.__cuaParity!.settings.showExperiments(x), experiments);
    await expect(page.locator('[data-settings-section="storage"]'), `Storage with ${JSON.stringify(experiments)}`).toHaveCount(sections.includes("storage") ? 1 : 0);
    checked++;
  }
  await navigate(page, "/settings/general");
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
