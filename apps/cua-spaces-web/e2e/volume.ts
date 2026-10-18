// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Screen checks for the Cua Volume flows: every state a replay reached
 * goes on its screen through `window.__cuaParity`, and the screen must show
 * what the core's view says. `drive-page` on the Volume page,
 * `drive-onboarding` on the first run's Cua Volume page, `driver-card` on
 * Settings' AI agents card.
 */

import { expect, type Page } from "@playwright/test";

import type { ParityCheckpoint } from "../src/bridge/parity";
import type { VolumeCheckpoint } from "../src/bridge/parity-volume";
import type { DriveView, LineView } from "../src/bridge/contracts/volume";
import type { OnboardingView } from "../src/bridge/contracts/onboarding";

const isVolume = (cp: ParityCheckpoint): cp is VolumeCheckpoint =>
  ["drive", "onboarding-drive", "drive-frame", "driver-frame", "driver-copy", "driver-summary"].includes(cp.kind);

const of = <K extends VolumeCheckpoint["kind"]>(cps: ParityCheckpoint[], kind: K) =>
  cps.filter(isVolume).filter((c): c is Extract<VolumeCheckpoint, { kind: K }> => c.kind === kind);

/** Each distinct state once. */
function distinct<T>(items: T[], key: (t: T) => unknown): T[] {
  const seen = new Set<string>();
  return items.filter((t) => {
    const k = JSON.stringify(key(t));
    if (seen.has(k)) return false;
    seen.add(k);
    return true;
  });
}

/* ---- The Volume page ------------------------------------------------------------ */

const line = (l: LineView, buttons: boolean) =>
  `${l.text} | ${l.trailing}${buttons ? ` | ${l.actionLabel ?? ""} / ${l.secondaryLabel ?? ""}` : ""}`;

/** What the Volume page must show for a core view. */
function expectedVolume(v: DriveView) {
  return {
    open: v.openLabel ? `${v.openLabel} ${v.mountPath ?? ""} ${v.busy ? "disabled" : "enabled"}` : null,
    mountLine: v.openLabel ? (v.mountLine ?? "") : null,
    requests: v.requests.map((l) => line(l, true)),
    grants: v.grants.length ? v.grants.map((l) => line(l, true)) : [v.grantsEmpty],
    devices: v.devices.map((l) => line(l, false)),
    syncNote: v.syncNote ? `${v.syncError ? "!" : ""}${v.syncNote}` : null,
    conflicts: v.conflicts.map((c) => `${c.text} | ${c.trailing} | ${c.openLabel && c.reveal ? `${c.openLabel} ${c.reveal}` : "-"} | ${c.resolveLabel}`),
    error: v.error ?? null,
  };
}

function readVolume(page: Page) {
  return page.evaluate(() => {
    const text = (el: Element | null) => el?.textContent?.trim() ?? "";
    const lines = (kind: string, buttons: boolean) =>
      [...document.querySelectorAll(`[data-drive-section="${kind}"] [data-drive-line]`)].map((el) => {
        const base = `${text(el.querySelector("[data-line-text]"))} | ${text(el.querySelector("[data-line-trailing]"))}`;
        return buttons ? `${base} | ${text(el.querySelector("[data-line-action]"))} / ${text(el.querySelector("[data-line-secondary]"))}` : base;
      });
    const open = document.querySelector<HTMLButtonElement>("[data-drive-open]");
    const note = document.querySelector("[data-drive-sync-note]");
    const empty = document.querySelector("[data-drive-grants-empty]");
    return {
      open: open ? `${text(open)} ${open.dataset.path ?? ""} ${open.disabled ? "disabled" : "enabled"}` : null,
      mountLine: open ? text(document.querySelector("[data-drive-mount-line]")) : null,
      requests: lines("requests", true),
      grants: empty ? [text(empty)] : lines("grants", true),
      devices: lines("devices", false),
      syncNote: note ? `${note.getAttribute("data-error") === "true" ? "!" : ""}${text(note)}` : null,
      conflicts: [...document.querySelectorAll("[data-drive-conflict]")].map((el) => {
        const o = el.querySelector<HTMLElement>("[data-conflict-open]");
        return `${text(el.querySelector("[data-line-text]"))} | ${text(el.querySelector("[data-line-trailing]"))} | ${o ? `${text(o)} ${o.dataset.conflictOpen}` : "-"} | ${text(el.querySelector("[data-conflict-resolve]"))}`;
      }),
      error: document.querySelector("[data-drive-error]")?.textContent ?? null,
    };
  });
}

async function checkVolumePage(page: Page, cps: ParityCheckpoint[]): Promise<number> {
  const states = distinct(of(cps, "drive"), (c) => [c.input, c.state]);
  for (const cp of states) {
    await page.evaluate(([input, state]) => window.__cuaParity!.showVolume(input, state), [cp.input, cp.state] as const);
    await expect.poll(() => readVolume(page), { message: `Volume page: ${cp.view.requestText ?? "idle"}` }).toEqual(expectedVolume(cp.view));
  }
  return states.length;
}

/* ---- The first run's Cua Volume page ------------------------------------------------ */

function expectedDriveCard(v: OnboardingView) {
  const c = v.drive!;
  return {
    title: v.title,
    label: c.label,
    checked: c.checked,
    enabled: c.enabled,
    note: c.note ?? null,
    error: c.error ?? null,
    settings: c.settingsUrl && c.settingsLabel ? c.settingsLabel : null,
    options: c.storageTitle ? c.storageOptions.map((o) => `${o.active ? "*" : ""}${o.label}`) : [],
    rows: c.storageRows.map((r) => `${r.id} ${r.kind}: ${r.label}`),
    storageNote: c.storageNote ?? null,
    storedIn: c.storedIn ?? null,
    mountedAt: c.mountedAt ?? null,
    preview: c.storageRows.length === 0,
    primary: `${v.primaryLabel} ${c.busy || !c.canContinue ? "disabled" : "enabled"}`,
  };
}

function readDriveCard(page: Page) {
  return page.evaluate(() => {
    const q = <T extends Element = Element>(s: string) => document.querySelector<T>(s);
    const text = (s: string) => q(s)?.textContent?.trim() ?? null;
    const toggle = q("[data-drive-toggle]");
    const primary = q<HTMLButtonElement>("[data-drive-continue]");
    return {
      title: text("h1"),
      label: text("[data-drive-label]"),
      checked: toggle?.getAttribute("aria-checked") === "true",
      enabled: !(toggle?.hasAttribute("data-disabled") ?? true),
      note: text("[data-drive-note]"),
      error: text("[data-drive-error]"),
      settings: text("[data-drive-settings]"),
      options: [...document.querySelectorAll<HTMLElement>("[data-storage-option]")].map((o) => `${o.dataset.active === "true" ? "*" : ""}${o.textContent?.trim()}`),
      rows: [...document.querySelectorAll<HTMLElement>("[data-drive-card] [data-row-id]")].map(
        (r) => `${r.dataset.rowId} ${r.dataset.rowKind}: ${r.querySelector("[data-row-label]")?.textContent?.trim() ?? ""}`,
      ),
      storageNote: text("[data-drive-storage-note]"),
      storedIn: text("[data-drive-stored]"),
      mountedAt: text("[data-drive-mounted]"),
      preview: q('[data-drive-card] [data-preview="drive"]') !== null,
      primary: primary ? `${primary.textContent?.trim()} ${primary.disabled ? "disabled" : "enabled"}` : null,
    };
  });
}

async function checkOnboardingDrive(page: Page, cps: ParityCheckpoint[]): Promise<number> {
  const states = distinct(of(cps, "onboarding-drive"), (c) => expectedDriveCard(c.view));
  let checked = 0;
  for (const cp of states) {
    await page.evaluate((state) => window.__cuaParity!.showOnboarding(state), cp.state);
    await expect.poll(() => readDriveCard(page), { message: `Cua Volume page: ${cp.view.primaryLabel}` }).toEqual(expectedDriveCard(cp.view));
    checked++;
  }
  // The miniature draws the core's frame at each sampled moment (on a page without the bucket's rows).
  const plain = states.find((c) => c.view.drive!.storageRows.length === 0);
  if (plain) {
    await page.evaluate((state) => window.__cuaParity!.showOnboarding(state), plain.state);
    for (const f of distinct(of(cps, "drive-frame"), (c) => c.tMs)) {
      await page.evaluate((t) => window.__cuaParity!.showDriver(t, null), f.tMs);
      const preview = page.locator('[data-drive-card] [data-preview="drive"]');
      await expect(preview).toHaveAttribute("data-volume", f.frame.volume.toFixed(3));
      await expect(preview).toHaveAttribute("data-arrived", f.frame.arrived.map((a) => a.toFixed(3)).join(","));
      checked++;
    }
  }
  return checked;
}

/* ---- The driver card ---------------------------------------------------------------------- */

async function checkDriverCard(page: Page, cps: ParityCheckpoint[]): Promise<number> {
  let checked = 0;
  const card = page.locator("[data-driver-card]").first();
  for (const c of of(cps, "driver-copy").slice(0, 1)) {
    await expect(card.locator("[data-driver-label]")).toHaveText(c.agentsDriver);
    await expect(card.getByRole("img")).toHaveAttribute("aria-label", c.agentsDriverImage);
    checked++;
  }
  for (const f of distinct(of(cps, "driver-frame"), (c) => c.tMs)) {
    await page.evaluate((t) => window.__cuaParity!.showDriver(t, null), f.tMs);
    const p = card.locator('[data-preview="driver"]');
    const fr = f.frame;
    await expect(p).toHaveAttribute("data-agent", `${fr.agent.x.toFixed(1)},${fr.agent.y.toFixed(1)}`);
    await expect(p).toHaveAttribute("data-pointer", `${fr.pointer.x.toFixed(1)},${fr.pointer.y.toFixed(1)}`);
    await expect(p).toHaveAttribute("data-pressed", String(fr.pressed));
    await expect(p).toHaveAttribute("data-agent-pressed", String(fr.agentPressed));
    await expect(p).toHaveAttribute("data-selection", fr.selection.toFixed(3));
    await expect(p).toHaveAttribute("data-ripple", fr.ripple.toFixed(3));
    await expect(p).toHaveAttribute("data-checked", fr.checked.map((x) => x.toFixed(3)).join(","));
    checked++;
  }
  const summaries = of(cps, "driver-summary").map((c) => c.summary);
  if (summaries.length) {
    await page.evaluate((s) => window.__cuaParity!.showDriver("still", s), summaries);
    const shown = () =>
      page.locator("[data-driver-summary]").evaluateAll((els) =>
        els.map((el) => ({
          line: el.querySelector("[data-summary-line]")?.textContent ?? "",
          text: el.querySelector("[data-summary-text]")?.textContent ?? "",
          failed: [...el.querySelectorAll("[data-summary-failed]")].map((f) => f.textContent ?? ""),
        })),
      );
    await expect.poll(shown).toEqual(summaries.map((s) => ({ line: s.line, text: s.text, failed: s.failed })));
    checked += summaries.length;
  }
  return checked;
}

/** The screen checks for one Cua Volume flow. */
export function checkVolumeFlow(page: Page, flow: string, cps: ParityCheckpoint[]): Promise<number> {
  switch (flow) {
    case "drive-page":
      return checkVolumePage(page, cps);
    case "drive-onboarding":
      return checkOnboardingDrive(page, cps);
    case "driver-card":
      return checkDriverCard(page, cps);
    default:
      throw new Error(`no Cua Volume checks for ${flow}`);
  }
}
