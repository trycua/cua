// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * New Space and "Connect a cloud" on the real screen: every distinct
 * wizard and sheet state a parity flow reached goes on screen through
 * `window.__cuaParity` (`showWizard`, `showCloudConnect`), and what the
 * screen draws must be what the core's view says: the step and its title,
 * Continue or Create and whether it can be pressed, the fields, tiles and
 * errors, the Run on menu entry by entry, the Resources lines, the
 * Summary, the address form, and the sheet's rows, fields and buttons.
 */

import { expect, type Locator, type Page } from "@playwright/test";

import type { CloudConnectView, WizardEnv, WizardState, WizardView } from "../src/bridge/contracts/new-space";
import type { ParityCheckpoint } from "../src/bridge/parity";

const texts = (l: Locator) => l.evaluateAll((els) => els.map((el) => (el.textContent ?? "").replace(/\s+/g, " ").trim()));
const attrs = (l: Locator, name: string) => l.evaluateAll((els, n) => els.map((el) => el.getAttribute(n) ?? ""), name);

const LINKS = new Set(["connect-cloud", "connect-by-address"]);

async function expectCheckbox(box: Locator, on: boolean, enabled = true): Promise<void> {
  await expect(box).toHaveAttribute("aria-checked", String(on));
  if (enabled) await expect(box).not.toHaveAttribute("data-disabled", "");
  else await expect(box).toHaveAttribute("data-disabled", "");
}

async function expectTiles(root: Locator, id: string, tiles: WizardView["osTiles"]): Promise<void> {
  const shown = root.locator(`[data-tile^="${id}:"]`);
  await expect(shown).toHaveCount(tiles.length);
  for (const t of tiles) {
    const tile = root.locator(`[data-tile="${id}:${t.id}"]`);
    await expect(tile).toHaveText(t.title);
    await expect(tile).toHaveAttribute("aria-pressed", String(t.pressed));
    if (t.enabled) await expect(tile).toBeEnabled();
    else await expect(tile).toBeDisabled();
  }
  // The chosen tile is filled, as the native segmented control's selection:
  // its background is not the unpressed tiles'.
  const pressed = tiles.find((t) => t.pressed);
  const other = tiles.find((t) => !t.pressed && t.enabled);
  if (pressed && other) {
    const bg = (t: { id: string }) => root.locator(`[data-tile="${id}:${t.id}"]`).evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(await bg(pressed)).not.toBe(await bg(other));
  }
}

/** The Run on menu, opened: each entry's id, label and whether it can be chosen. */
async function expectRunOnMenu(page: Page, v: WizardView): Promise<void> {
  await page.locator("[data-run-on]").click();
  const items = page.locator("[data-run-on-option]:visible");
  await expect(items).toHaveCount(v.placements.length);
  expect(await attrs(items, "data-value")).toEqual(v.placements.map((o) => o.id));
  expect(await items.evaluateAll((els) => els.map((el) => el.hasAttribute("data-disabled")))).toEqual(v.placements.map((o) => !o.enabled));
  const labels = await items.evaluateAll((els) => els.map((el) => el.querySelector("[data-item-label]")?.textContent ?? ""));
  expect(labels).toEqual(v.placements.map((o) => o.label));
  await page.keyboard.press("Escape");
  await expect(items).toHaveCount(0);
}

async function checkWizard(page: Page, state: WizardState, v: WizardView, menuChanged: boolean): Promise<void> {
  const root = page.locator("[data-new-space]");
  await expect(root).toHaveAttribute("data-mode", v.mode);
  await expect(root).toHaveAttribute("data-step", String(v.step));
  await expect(root.locator("[data-wizard-title]")).toHaveText(v.title);

  const primary = root.locator("[data-wizard-primary]");
  const back = root.locator("[data-wizard-back]");
  await expect(root.locator("[data-wizard-cancel]")).toHaveText(v.labels.cancel);

  if (v.mode === "address") {
    await expect(primary).toHaveText(v.address.submitLabel);
    if (v.address.canSubmit) await expect(primary).toBeEnabled();
    else await expect(primary).toBeDisabled();
    await expect(back).toHaveText(v.labels.back);
    const address = state.address ?? { url: "", token: "", name: "" };
    await expect(root.locator('[data-address-input="address"]')).toHaveValue(address.url);
    await expect(root.locator('[data-address-input="token"]')).toHaveValue(address.token);
    await expect(root.locator('[data-address-input="address-name"]')).toHaveValue(address.name);
    await expect(root.locator('[data-address-input="address"]')).toHaveAttribute("aria-invalid", String(v.address.showInvalid));
    if (v.address.error) await expect(root.locator("[data-address-error]")).toHaveText(v.address.error);
    else await expect(root.locator("[data-address-error]")).toHaveCount(0);
    return;
  }

  await expect(primary).toHaveText(v.primaryLabel);
  if (v.step >= 3 || v.canContinue) await expect(primary).toBeEnabled();
  else await expect(primary).toBeDisabled();
  await expect(back).toHaveCount(v.showBack ? 1 : 0);
  // The step list: one entry per step, the current one marked.
  expect(await attrs(root.locator("[data-step-state]"), "data-step-state")).toEqual(v.steps.map((s) => s.state));

  const shown = v.fields.filter((f) => !f.advanced || v.advanced);
  const field = (id: string) => shown.find((f) => f.id === id);

  if (v.step === 0) {
    expect((await attrs(root.locator("[data-field]"), "data-field")).sort()).toEqual(shown.map((f) => f.id).sort());
    const errors: Record<string, string> = {};
    for (const f of shown) {
      if (f.id === "image") {
        if (v.imageField.error) errors.image = v.imageField.error;
      } else if (f.error && !LINKS.has(f.id)) errors[f.id] = f.error;
    }
    const errorEls = root.locator("[data-field-error]");
    const drawn = Object.fromEntries((await attrs(errorEls, "data-field-error")).map((id, i) => [id, i]));
    const errorTexts = await texts(errorEls);
    expect(Object.fromEntries(Object.entries(drawn).map(([id, i]) => [id, errorTexts[i]!]))).toEqual(errors);

    const advanced = v.fields.some((f) => f.advanced);
    await expect(root.locator("[data-advanced-toggle]")).toHaveCount(advanced ? 1 : 0);
    if (advanced) await expect(root.locator("[data-advanced-toggle]")).toHaveAttribute("aria-expanded", String(v.advanced));

    if (field("os")) await expectTiles(root, "os", v.osTiles);
    if (field("kind")) await expectTiles(root, "kind", v.kindTiles);
    if (field("image")) {
      const input = root.locator("[data-image-input]");
      await expect(input).toHaveValue(v.imageField.text);
      await expect(input).toHaveAttribute("aria-expanded", String(v.imageField.open));
      const rows = v.imageField.open ? v.imageField.groups.flatMap((g) => g.rows) : [];
      const drawnRows = root.locator("[data-image-row]");
      expect(await attrs(drawnRows, "data-image-row")).toEqual(rows.map((r) => r.ref));
      expect(await attrs(drawnRows, "aria-selected")).toEqual(rows.map((r) => String(r.highlighted)));
    }
    if (field("placement")) {
      const chosen = v.placements.find((o) => o.selected);
      await expect(root.locator("[data-run-on]")).toHaveText(chosen?.label ?? "");
      if (menuChanged) await expectRunOnMenu(page, v);
    }
    if (field("runtime")) {
      const runtime = root.locator("[data-runtime]");
      await expect(runtime).toHaveText(v.runtimes.find((r) => r.value === v.runtime)?.label ?? "");
      if (v.runtimeEnabled) await expect(runtime).not.toHaveAttribute("data-disabled", "");
      else await expect(runtime).toHaveAttribute("data-disabled", "");
    }
    if (v.placementHint) await expect(root.locator("[data-placement-hint]")).toHaveText(v.placementHint);
  } else if (v.step === 1) {
    const value = (id: string) => root.locator(`[data-value-text="${id}"]`);
    if (field("cpus")) await expect(value("cpus")).toHaveText(v.cpusText);
    else await expect(value("cpus")).toHaveCount(0);
    if (field("memory")) await expect(value("memory")).toHaveText(v.memoryText);
    else await expect(value("memory")).toHaveCount(0);
    if (v.diskEditable) {
      await expect(value("disk")).toHaveText(v.diskText);
      if (v.diskNote) await expect(root.locator("[data-disk-note]")).toHaveText(v.diskNote);
      await expect(root.locator("[data-disk-reset]")).toHaveCount(v.diskResetLabel ? 1 : 0);
    } else await expect(value("disk")).toHaveCount(0);
    if (v.gpu) {
      await expect(root.locator("[data-gpu-label]")).toHaveText(v.gpu.label);
      await expectCheckbox(root.locator("[data-gpu]"), v.gpu.on, v.gpu.enabled);
      // The label names the checkbox; why it is off only describes it.
      await expect(root.locator("[data-gpu]")).toHaveAccessibleName(v.gpu.label);
      if (!v.gpu.enabled && v.gpu.reason) {
        await expect(root.locator("[data-gpu-reason]")).toHaveText(v.gpu.reason);
        await expect(root.locator("[data-gpu]")).toHaveAccessibleDescription(v.gpu.reason);
      }
    } else await expect(root.locator("[data-gpu]")).toHaveCount(0);
    if (v.price) await expect(root.locator("[data-price]")).toHaveText(v.price);
    else await expect(root.locator("[data-price]")).toHaveCount(0);
    const facts = root.locator("[data-fact]");
    expect(await attrs(facts, "data-fact")).toEqual(v.resourceFacts.map((f) => f.id));
    expect(await texts(root.locator("[data-fact] [data-fact-value]"))).toEqual(v.resourceFacts.map((f) => f.value));
    if (v.resourcesError) await expect(root.locator("[data-resources-error]")).toHaveText(v.resourcesError);
    else await expect(root.locator("[data-resources-error]")).toHaveCount(0);
  } else if (v.step === 2) {
    await expect(root.locator("[data-name-input]")).toHaveValue(v.name);
    await expect(root.locator("[data-name-input]")).toHaveAttribute("aria-invalid", String(v.nameInvalid));
    if (v.nameError) await expect(root.locator('[data-field-error="name"]')).toHaveText(v.nameError);
    else await expect(root.locator('[data-field-error="name"]')).toHaveCount(0);
    await expectCheckbox(root.locator("[data-open-when-ready]"), v.openWhenReady);
    if (v.streamNote) await expect(root.locator("[data-stream-note]")).toHaveText(v.streamNote);
    else await expect(root.locator("[data-stream-note]")).toHaveCount(0);
  } else {
    const rows = await root.locator("[data-summary-row]").evaluateAll((els) => els.map((el) => [...el.children].map((c) => c.textContent ?? "")));
    expect(rows).toEqual(v.summary.map((f) => [f.label, f.value]));
  }
}

async function checkCloudConnect(page: Page, v: CloudConnectView): Promise<void> {
  const root = page.locator("[data-connect-cloud]");
  await expect(root.locator("[data-cloud-title]")).toHaveText(v.title);
  const rows = root.locator("[data-cloud-row]");
  expect(await attrs(rows, "data-cloud-row")).toEqual(v.rows.map((r) => r.id));
  expect(await attrs(rows, "aria-checked")).toEqual(v.rows.map((r) => String(r.selected)));
  expect(await texts(root.locator("[data-cloud-row-title]"))).toEqual(v.rows.map((r) => r.title));
  expect(await texts(root.locator("[data-cloud-row-detail]"))).toEqual(v.rows.map((r) => r.detail));
  expect(await rows.evaluateAll((els) => els.map((el) => el.querySelector("[data-cloud-found]") !== null))).toEqual(v.rows.map((r) => r.found));

  const fields = [v.field, v.profileField].filter((f) => f !== null);
  const drawn = root.locator("[data-cloud-field]");
  expect(await attrs(drawn, "data-cloud-field")).toEqual(fields.map((f) => f.id));
  expect(await drawn.locator("input").evaluateAll((els) => els.map((el) => (el as HTMLInputElement).value))).toEqual(fields.map((f) => f.value));
  if (v.field) await expectCheckbox(root.locator("[data-make-default]"), v.makeDefault);
  else await expect(root.locator("[data-make-default]")).toHaveCount(0);

  expect(await texts(root.locator("[data-cloud-touches] p"))).toEqual(v.touches);
  expect(await attrs(root.locator("[data-cloud-check]"), "data-cloud-check")).toEqual(v.checks.map((c) => (c.ok ? "ok" : "failed")));
  expect(await texts(root.locator("[data-cloud-check]"))).toEqual(v.checks.map((c) => c.text));
  if (v.result) await expect(root.locator("[data-cloud-result]")).toHaveText(v.result);
  else await expect(root.locator("[data-cloud-result]")).toHaveCount(0);
  if (v.error) await expect(root.locator("[data-cloud-error]")).toHaveText(v.error);
  else await expect(root.locator("[data-cloud-error]")).toHaveCount(0);

  await expect(root.locator("[data-cloud-cancel]")).toHaveText(v.cancelLabel);
  const test = root.locator("[data-cloud-test]");
  await expect(test).toHaveText(v.testLabel);
  if (v.canTest) await expect(test).toBeEnabled();
  else await expect(test).toBeDisabled();
  const connect = root.locator("[data-cloud-connect]");
  await expect(connect).toHaveText(v.connectLabel);
  if (v.canConnect) await expect(connect).toBeEnabled();
  else await expect(connect).toBeDisabled();
}

/** Puts each distinct New Space and sheet state on screen and checks it. */
export async function checkNewSpace(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  const seen = new Set<string>();
  let lastMenu = "";
  for (const cp of checkpoints) {
    if (cp.kind === "wizard") {
      const key = JSON.stringify(["wizard", cp.view, cp.state.address ?? null]);
      if (seen.has(key)) continue;
      seen.add(key);
      await page.evaluate(([state, env]) => window.__cuaParity!.showWizard(state, env), [cp.state, cp.env] as [WizardState, WizardEnv]);
      const menu = JSON.stringify(cp.view.placements);
      // The open image suggestions cover the menu; it is checked on a later state.
      const menuChanged = cp.view.mode === "create" && cp.view.step === 0 && !cp.view.imageField.open && menu !== lastMenu;
      await checkWizard(page, cp.state, cp.view, menuChanged);
      if (menuChanged && cp.view.fields.some((f) => f.id === "placement")) lastMenu = menu;
    } else if (cp.kind === "cloud-connect") {
      const key = JSON.stringify(["sheet", cp.view]);
      if (seen.has(key)) continue;
      seen.add(key);
      await page.evaluate(([input, state]) => window.__cuaParity!.showCloudConnect(input, state), [cp.input, cp.state] as const);
      await checkCloudConnect(page, cp.view);
    }
  }
  return seen.size;
}
