// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Screen checks for the `agent-keys` flow (Settings, Connections, Providers): each section,
 * sheet and Remove question the replay reached goes on the real screen
 * through `window.__cuaParity.showAgentKeys`, and the screen must show the
 * core's words. Then a key is added and removed for real on the demo host,
 * and the key never shows up in the page.
 */

import { expect, type Page } from "@playwright/test";

import type { ParityCheckpoint } from "../src/bridge/parity";
import type { AgentKeysCheckpoint } from "../src/bridge/parity-agent-keys";

type Of<K extends AgentKeysCheckpoint["kind"]> = Extract<AgentKeysCheckpoint, { kind: K }>;

const unique = <K extends AgentKeysCheckpoint["kind"]>(cps: ParityCheckpoint[], kind: K): Of<K>[] => {
  const seen = new Set<string>();
  return cps.filter((c): c is Of<K> => {
    if (c.kind !== kind) return false;
    const key = JSON.stringify(c);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
};

/** A key the checks type; it must never be drawn. */
const TYPED = "sk-ant-test-0000";

export async function checkAgentKeys(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  let checked = 0;
  await page.goto("/settings/connections?bridge=demo&parity");
  await page.waitForFunction(() => window.__cuaParity !== undefined);
  expect(await page.evaluate(() => window.__cuaParity!.coreStatus), "the wasm core loaded").toBe("ready");

  for (const { input, view } of unique(checkpoints, "agent-keys")) {
    await page.evaluate((i) => window.__cuaParity!.showAgentKeys(i), input);
    const step = `Settings, Connections with ${input.keys.map((k) => k.env).join(", ") || "no keys"}${view.notice ? " (notice)" : ""}`;
    // The Providers group draws the core's view of the same keys: the add button follows canEdit.
    await expect(page.locator("[data-connectors-providers]"), step).toBeVisible();
    if (view.canEdit) await expect(page.locator("[data-agent-key-add-other]"), step).toBeEnabled();
    else await expect(page.locator("[data-agent-key-add-other]"), step).toBeDisabled();
    checked++;
  }

  for (const { input, form, view } of unique(checkpoints, "agent-key-form")) {
    const env = form.env ?? null;
    await page.evaluate(([i, sheet]) => window.__cuaParity!.showAgentKeys(i, { sheet }), [input, { provider: form.provider, env }] as const);
    const step = `the sheet: ${form.provider}${env ? ` ${env}` : ""}${form.name ? ` named ${form.name}` : ""}${form.hasValue ? ", a key typed" : ""}`;
    const sheet = page.locator("[data-agent-key-sheet]");
    await expect(sheet, step).toBeVisible();
    if (form.name) await sheet.locator("[data-agent-key-name]").fill(form.name);
    if (form.hasValue) await sheet.locator("[data-agent-key-value]").fill(TYPED);
    await expect(sheet.locator("[data-agent-key-sheet-title]"), step).toHaveText(view.title);
    await expect(sheet.locator("[data-agent-key-sheet-lede]"), step).toHaveText(view.lede);
    await expect(sheet.locator("[data-agent-key-name]"), step).toHaveCount(view.nameLabel ? 1 : 0);
    if (view.namePlaceholder) await expect(sheet.locator("[data-agent-key-name]"), step).toHaveAttribute("placeholder", view.namePlaceholder);
    if (view.nameError) await expect(sheet.locator("[data-agent-key-name-error]"), step).toHaveText(view.nameError);
    else await expect(sheet.locator("[data-agent-key-name-error]"), step).toHaveCount(0);
    const value = sheet.locator("[data-agent-key-value]");
    await expect(value, step).toHaveAttribute("type", "password");
    await expect(value, step).toHaveAttribute("placeholder", view.valuePlaceholder);
    await expect(sheet.locator("[data-agent-key-value-help]"), step).toHaveText(view.valueHelp);
    await expect(sheet.locator("[data-agent-key-save]"), step).toHaveText(view.saveLabel);
    await expect(sheet.locator("[data-agent-key-cancel]"), step).toHaveText(view.cancelLabel);
    if (view.canSave) await expect(sheet.locator("[data-agent-key-save]"), step).toBeEnabled();
    else await expect(sheet.locator("[data-agent-key-save]"), step).toBeDisabled();
    await sheet.locator("[data-agent-key-cancel]").click();
    await expect(sheet, step).toHaveCount(0);
    checked++;
  }

  for (const { input, env, confirm } of unique(checkpoints, "agent-key-remove")) {
    await page.evaluate(([i, removing]) => window.__cuaParity!.showAgentKeys(i, { removing }), [input, env] as const);
    const step = `removing ${env}`;
    const dialog = page.getByRole("alertdialog");
    if (!confirm) {
      await expect(dialog, step).toHaveCount(0);
    } else {
      await expect(dialog.getByText(confirm.title), step).toBeVisible();
      await expect(dialog.getByText(confirm.message), step).toBeVisible();
      await expect(dialog.getByRole("button", { name: confirm.confirmLabel }), step).toBeVisible();
      await dialog.getByRole("button", { name: confirm.cancelLabel }).click();
      await expect(dialog, step).toHaveCount(0);
    }
    checked++;
  }

  // For real on the demo host: add another company's key, then remove it.
  await page.goto("/settings/connections?bridge=demo");
  await page.locator("[data-agent-key-add-other]").click();
  const sheet = page.locator("[data-agent-key-sheet]");
  await expect(sheet.locator("[data-agent-key-save]")).toBeDisabled();
  await sheet.locator("[data-agent-key-name]").fill("ACME_API_KEY");
  await sheet.locator("[data-agent-key-value]").fill(TYPED);
  await sheet.locator("[data-agent-key-save]").click();
  await expect(sheet).toHaveCount(0);
  const acme = page.locator('[data-agent-key-row="ACME_API_KEY"]');
  await expect(acme).toContainText("•••• 0000");
  expect(await page.content(), "the key is never drawn").not.toContain(TYPED);
  await acme.getByRole("button", { name: "Remove" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Remove" }).click();
  await expect(acme).toHaveCount(0);
  checked++;
  return checked;
}
