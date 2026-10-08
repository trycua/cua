// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The teleport picker, its grid and the Share sheet against the states the
 * parity replay reached (`src/bridge/parity-teleport.ts`). Each state goes
 * on the real screen through the bridge, and the screen must draw what the
 * core says it draws: the grid's tiles in the core's own line format
 * (`grid_frame` in the core's parity.rs), each picker step's controls, the
 * review's items and gates, and every line of the Share sheet.
 */

import { expect, type Page } from "@playwright/test";

import type { ParityCheckpoint } from "../src/bridge/parity";
import type { PickerGrid, PickerTile } from "../src/bridge/contracts/teleport";
import type { GridShown } from "../src/bridge/parity-teleport";

type Cp<K extends ParityCheckpoint["kind"]> = Extract<ParityCheckpoint, { kind: K }>;

const once = <T>(items: T[], key: (t: T) => unknown): T[] => {
  const seen = new Set<string>();
  return items.filter((t) => {
    const k = JSON.stringify(key(t));
    if (seen.has(k)) return false;
    seen.add(k);
    return true;
  });
};

/* ---- The grid ---------------------------------------------------------------- */

function tileLine(t: PickerTile): string {
  const icon =
    t.icon.kind === "host" ? `host ${t.icon.path}` : t.icon.kind === "guest" ? `guest ${t.icon.appName} ${t.icon.appId} ${t.icon.pid}` : "-";
  const thumb =
    t.thumbnail.kind === "host-window"
      ? `window ${t.thumbnail.windowId}`
      : t.thumbnail.kind === "guest-window"
        ? `guest ${t.thumbnail.windowId}@${t.thumbnail.epoch}`
        : "-";
  return `${t.id} | ${t.title} | ${icon} | ${thumb} | ${t.help}${t.disabled ? " (dim)" : ""}${t.selected ? " (selected)" : ""}`;
}

/** The core's grid as `grid_frame` writes it. */
function gridLines(g: PickerGrid): { tiles: string[]; emptyText: string | null } {
  const tiles: string[] = [];
  for (const s of g.sections) {
    if (s.title) tiles.push(`# ${s.title}`);
    for (const t of s.tiles) tiles.push(tileLine(t));
  }
  return { tiles, emptyText: g.emptyText };
}

/** The grid on screen, in the same format. */
const readGrid = (page: Page) =>
  page.locator("[data-teleport]").evaluate((root) => {
    const tiles: string[] = [];
    for (const sec of root.querySelectorAll("[data-grid-section]")) {
      const title = sec.getAttribute("data-grid-section") ?? "";
      if (title) tiles.push(`# ${title}`);
      for (const el of sec.querySelectorAll("[data-tile]")) {
        tiles.push(
          `${el.getAttribute("data-tile")} | ${el.querySelector("[data-tile-title]")?.textContent ?? ""} | ${el.getAttribute("data-icon")} | ${el.getAttribute("data-thumbnail")} | ${el.getAttribute("title") ?? ""}${el.hasAttribute("data-disabled") ? " (dim)" : ""}${el.hasAttribute("data-selected") ? " (selected)" : ""}`,
        );
      }
    }
    return { tiles, emptyText: root.querySelector("[data-grid-empty]")?.textContent ?? null };
  });

async function showGrid(page: Page, shown: GridShown): Promise<void> {
  await page.evaluate((s) => window.__cuaParity!.showTeleportGrid(s), shown);
  await expect(page.locator("[data-teleport][data-step=pick]")).toBeVisible();
}

/* ---- Picker steps -------------------------------------------------------------- */

async function checkPicker(page: Page, cp: Cp<"picker">): Promise<void> {
  const { state, frame } = cp;
  await page.evaluate((s) => window.__cuaParity!.showTeleport(s), state);
  const root = page.locator(`[data-teleport][data-step=${state.step}]`);
  await expect(root, `picker at ${state.step}`).toBeVisible();
  switch (state.step) {
    case "loading":
    case "planning":
      await expect(root.locator("[data-teleport-busy]")).toBeVisible();
      break;
    case "pick": {
      // The Apps tab draws the core's sections, its names in order, and the highlight.
      const want = frame.sections.map((s) => `${s.title}: ${s.entries.map((e) => e.name).join(", ")}`);
      await expect
        .poll(() =>
          root.locator("[data-grid-section]").evaluateAll((els) =>
            els.map((el) => `${el.getAttribute("data-grid-section")}: ${[...el.querySelectorAll("[data-tile-title]")].map((t) => t.textContent).join(", ")}`),
          ),
        )
        .toEqual(want);
      const visible = frame.sections.flatMap((s) => s.entries.map((e) => e.id));
      const selected = state.selectedId && visible.includes(state.selectedId) ? [state.selectedId] : [];
      await expect.poll(() => root.locator("[data-tile][data-selected]").evaluateAll((els) => els.map((e) => e.getAttribute("data-tile")))).toEqual(selected);
      break;
    }
    case "options": {
      await expect.poll(() => root.locator("[data-move]").evaluateAll((els) => els.map((e) => e.getAttribute("data-move")))).toEqual(state.entry?.moves ?? []);
      await expect(root.locator("[data-move]:has([data-checked])")).toHaveAttribute("data-move", state.move ?? "app_only");
      const want = frame.sensitive.map((o) => `${o.group}:${o.label}:${o.detail}:${o.checked ? "on" : "off"}`);
      await expect
        .poll(() =>
          root.locator("[data-sensitive]").evaluateAll((els) =>
            els.map(
              (el) =>
                `${el.getAttribute("data-sensitive")}:${el.querySelector("[data-sensitive-label]")?.textContent}:${el.querySelector("[data-sensitive-detail]")?.textContent}:${el.hasAttribute("data-checked") ? "on" : "off"}`,
            ),
          ),
        )
        .toEqual(want);
      if (frame.canPlan) await expect(page.locator("[data-teleport-plan]")).toBeEnabled();
      else await expect(page.locator("[data-teleport-plan]")).toBeDisabled();
      break;
    }
    case "consent": {
      const r = frame.review!;
      await expect(page.locator("[data-teleport-title]")).toHaveText(r.title);
      // Every install and every line the user can turn off, with secrets marked.
      const kind = (i: { kind: string; sensitive: boolean }) => (i.kind === "install" ? "install" : i.sensitive ? "secret" : "state");
      const shown = r.items.filter((i) => i.kind === "install" || r.toggles.some((t) => t.key === i.key));
      await expect
        .poll(() =>
          root.locator("[data-review-item]").evaluateAll((els) =>
            els.map((el) => `${el.getAttribute("data-kind")}:${el.querySelector("[data-review-label]")?.textContent}`).sort(),
          ),
        )
        .toEqual(shown.map((i) => `${kind(i)}:${i.label}`).sort());
      if (r.leavesText) await expect(page.locator("[data-review-leaves]")).toHaveText(r.leavesText);
      else await expect(page.locator("[data-review-leaves]")).toHaveCount(0);
      await expect.poll(() => root.locator("[data-review-warning]").allTextContents()).toEqual(r.warnings);
      for (const [gate, needed, checked] of [
        ["ack", r.needsAcknowledgement, r.acknowledged],
        ["ack-relay", r.needsRelayPlaintextAcknowledgement, r.acknowledgedRelayPlaintext],
      ] as const) {
        const box = root.locator(`[data-gate=${gate}]`);
        await expect(box, gate).toHaveCount(needed ? 1 : 0);
        if (needed) {
          if (checked) await expect(box).toHaveAttribute("data-checked", "true");
          else await expect(box).not.toHaveAttribute("data-checked");
        }
      }
      if (r.canConfirm) await expect(page.locator("[data-teleport-confirm]")).toBeEnabled();
      else await expect(page.locator("[data-teleport-confirm]")).toBeDisabled();
      break;
    }
    case "running":
      await expect(root.locator("[data-teleport-progress]")).toHaveAttribute("data-teleport-progress", String(Math.round(frame.progress * 1000)));
      break;
    case "done":
      await expect(root.locator("[data-teleport-done]")).toBeVisible();
      break;
    case "error":
      await expect(root.locator("[data-teleport-error]")).toHaveText(state.installPrompt?.message ?? state.error ?? "");
      break;
  }
}

/** Every teleport checkpoint on screen; returns how many states were checked. */
export async function checkTeleport(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  let checked = 0;
  const pickers = once(
    checkpoints.filter((c): c is Cp<"picker"> => c.kind === "picker"),
    (c) => c.state,
  );
  for (const cp of pickers) {
    await checkPicker(page, cp);
    checked++;
  }
  for (const cp of once(checkpoints.filter((c): c is Cp<"grid-tabs"> => c.kind === "grid-tabs"), (c) => c.tabs)) {
    await showGrid(page, { tab: "apps", spaceName: cp.spaceName });
    await expect
      .poll(() => page.locator("[data-teleport-tab]").evaluateAll((els) => els.map((e) => `${e.getAttribute("data-teleport-tab")}=${e.textContent}`)))
      .toEqual(cp.tabs.map((t) => `${t.tab}=${t.label}`));
    checked++;
  }
  const grids = once(
    checkpoints.filter((c): c is Cp<"grid"> => c.kind === "grid"),
    (c) => [c.shown, c.grid],
  );
  for (const cp of grids) {
    await showGrid(page, cp.shown);
    await expect.poll(() => readGrid(page), { message: `${cp.shown.tab} grid` }).toEqual(gridLines(cp.grid));
    checked++;
  }
  // Each tab's primary button, on the grid it was asked for.
  for (const cp of checkpoints.filter((c): c is Cp<"grid-primary"> => c.kind === "grid-primary")) {
    const source = grids.find((g) => g.shown.tab === cp.tab && JSON.stringify(g.grid) === JSON.stringify(cp.grid));
    expect(source, `the ${cp.tab} grid its primary button was asked for`).toBeDefined();
    await showGrid(page, { ...source!.shown, spaceName: cp.spaceName });
    const button = page.locator("[data-teleport-primary]");
    await expect(button).toHaveText(cp.primary.label);
    if (cp.primary.enabled) await expect(button).toBeEnabled();
    else await expect(button).toBeDisabled();
    checked++;
  }
  return checked;
}

/* ---- The Share sheet ------------------------------------------------------------ */

export async function checkShare(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  let checked = 0;
  const shares = once(
    checkpoints.filter((c): c is Cp<"share"> => c.kind === "share"),
    (c) => [c.input, c.state],
  );
  for (const { input, state, view: v } of shares) {
    await page.evaluate(([i, s]) => window.__cuaParity!.showShare(i, s), [input, state] as const);
    const sheet = page.locator("[data-share-sheet]");
    await expect(sheet.locator("[data-share-title]")).toHaveText(v.title);
    await expect
      .poll(() =>
        sheet
          .locator("[data-share-row]")
          .evaluateAll((els) =>
            els.map((el) => `${el.getAttribute("data-share-row")} | ${el.getAttribute("data-role")}${el.hasAttribute("data-connected") ? " | connected" : ""}`),
          ),
      )
      .toEqual(v.rows.map((r) => `${r.who} | ${r.role}${r.connected ? " | connected" : ""}`));
    if (v.rows.length === 0) await expect(sheet.locator("[data-share-empty]")).toHaveText(v.emptyText);
    const who = sheet.locator("[data-share-who]");
    await expect(who).toHaveValue(v.who);
    await expect(who).toHaveAttribute("placeholder", v.whoPlaceholder);
    await expect(sheet.locator("[data-share-role]")).toHaveAttribute("data-share-role", v.role);
    const submit = sheet.locator("[data-share-submit]");
    await expect(submit).toHaveText(v.shareLabel);
    if (v.canShare) await expect(submit).toBeEnabled();
    else await expect(submit).toBeDisabled();
    for (const [sel, text] of [
      ["[data-share-hint]", v.hint],
      ["[data-share-error]", v.error],
      ["[data-share-disabled]", v.disabledReason],
    ] as const) {
      if (text) await expect(sheet.locator(sel)).toHaveText(text);
      else await expect(sheet.locator(sel)).toHaveCount(0);
    }
    // Nobody can be removed while a request runs.
    for (const remove of await sheet.locator("[data-share-remove]").all()) {
      if (v.busy) await expect(remove).toBeDisabled();
      else await expect(remove).toBeEnabled();
    }
    checked++;
  }
  return checked;
}
