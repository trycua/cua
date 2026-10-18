// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Screen checks for the `agent-keys` flow (Settings → Agents): each section,
 * sheet and Remove question the replay reached goes on the real screen
 * through `window.__cuaParity.showAgentKeys`, and the screen must show the
 * core's words. Then a key is added and removed for real on the demo host,
 * and the key never shows up in the page.
 */

import { expect, type Locator, type Page } from "@playwright/test";

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

const texts = (l: Locator) => l.evaluateAll((els) => els.map((el) => (el.textContent ?? "").trim()));
const attrs = (l: Locator, name: string) => l.evaluateAll((els, n) => els.map((el) => el.getAttribute(n) ?? ""), name);

/** A key the checks type; it must never be drawn. */
const TYPED = "sk-ant-test-0000";

export async function checkAgentKeys(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  let checked = 0;

  for (const { input, view } of unique(checkpoints, "agent-keys")) {
    await page.evaluate((i) => window.__cuaParity!.showAgentKeys(i), input);
    const step = `Settings → Agents with ${input.keys.map((k) => k.env).join(", ") || "no keys"}${view.notice ? " (notice)" : ""}`;
    const rows = page.locator("[data-agent-key-row]");
    await expect.poll(() => attrs(rows, "data-agent-key-row"), { message: step }).toEqual(view.rows.map((r) => r.env));
    expect(await texts(page.locator("[data-agent-key-title]")), step).toEqual(view.rows.map((r) => r.title));
    expect(await texts(page.locator("[data-agent-key-status]")), step).toEqual(view.rows.map((r) => r.status));
    expect(await texts(page.locator("[data-agent-key-detail]")), step).toEqual(view.rows.map((r) => r.detail));
    expect(await texts(page.locator("[data-agent-key-action]")), step).toEqual(view.rows.map((r) => r.actionLabel));
    expect(await texts(page.locator("[data-agent-key-remove]")), step).toEqual(view.rows.flatMap((r) => (r.removeLabel ? [r.removeLabel] : [])));
    // The date is the page's; the label before it is the core's.
    const added = await texts(page.locator("[data-agent-key-added]"));
    expect(added.length, step).toBe(view.rows.filter((r) => r.addedMs !== null).length);
    for (const a of added) expect(a.startsWith(`${view.addedLabel} `), step).toBe(true);
    await expect(page.locator("[data-agent-keys-intro]"), step).toHaveText(view.intro);
    await expect(page.locator("[data-agent-keys-title]"), step).toHaveText(view.title);
    await expect(page.locator("[data-agent-key-add-other]"), step).toHaveText(view.addOtherLabel);
    await expect(page.locator("[data-agent-key-other-help]"), step).toHaveText(view.otherHelp);
    if (view.notice) await expect(page.locator("[data-agent-keys-notice]"), step).toContainText(view.notice);
    else await expect(page.locator("[data-agent-keys-notice]"), step).toHaveCount(0);
    for (const b of await page.locator("[data-agent-key-action], [data-agent-key-add-other]").all()) {
      if (view.canEdit) await expect(b, step).toBeEnabled();
      else await expect(b, step).toBeDisabled();
    }
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

  // For real on the demo host: add an Anthropic key, then remove it.
  await page.goto("/settings/agents?bridge=demo");
  const anthropic = page.locator('[data-agent-key-row="ANTHROPIC_API_KEY"]');
  await expect(anthropic.locator("[data-agent-key-status]")).toHaveText("Not set");
  await anthropic.locator("[data-agent-key-action]").click();
  const sheet = page.locator("[data-agent-key-sheet]");
  await expect(sheet.locator("[data-agent-key-save]")).toBeDisabled();
  await sheet.locator("[data-agent-key-value]").fill(TYPED);
  await sheet.locator("[data-agent-key-save]").click();
  await expect(sheet).toHaveCount(0);
  await expect(anthropic.locator("[data-agent-key-status]")).toHaveText("•••• 0000");
  expect(await page.content(), "the key is never drawn").not.toContain(TYPED);
  await anthropic.locator("[data-agent-key-remove]").click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Remove" }).click();
  await expect(anthropic.locator("[data-agent-key-status]")).toHaveText("Not set");
  checked++;
  return checked;
}
