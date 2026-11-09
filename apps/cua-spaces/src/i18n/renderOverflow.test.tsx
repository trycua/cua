// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Render check for overflow in every language. jsdom has no layout, so this
// measures what would overflow a control from the text itself: every label
// on key screens must be translated (no raw key, no ICU left over) and fit a
// display-width budget, counting CJK characters as two columns. Real pixels
// are checked by the Playwright parity run in apps/cua-spaces-web once that
// UI is on i18n.
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createFakeHostBridge } from "../native/host";
import { Onboarding } from "../components/Onboarding";
import { SettingsPanel } from "../components/SettingsPanel";
import i18n, { AVAILABLE_LOCALES } from "./index";

/** Terminal-style columns: wide (CJK, full-width) characters take two. */
function columns(text: string): number {
  let n = 0;
  for (const ch of text) n += /[ᄀ-ᅟ⺀-꓏가-힣豈-﫿︰-﹯＀-｠￠-￦]/u.test(ch) ? 2 : 1;
  return n;
}

/** What one control (button, tab, option, heading, label) may spend on its text. */
const CONTROL_BUDGET = 48;

function controlTexts(root: HTMLElement): string[] {
  const nodes = root.querySelectorAll("button, [role=radio], [role=tab], option, h1, h2, h3, label, .st-label");
  return [...nodes].map((n) => (n.textContent ?? "").trim()).filter(Boolean);
}

beforeEach(() => window.localStorage.clear());
afterEach(async () => {
  cleanup();
  await act(async () => {
    await i18n.changeLanguage("en");
  });
});

describe.each(AVAILABLE_LOCALES as readonly string[])("render overflow: %s", (lng) => {
  const screens: Record<string, () => ReturnType<typeof render>> = {
    onboarding: () => render(<Onboarding host={createFakeHostBridge()} onDone={vi.fn()} />),
    settings: () =>
      render(<SettingsPanel menuBar={false} onMenuBarMode={vi.fn()} onOpenExternal={vi.fn()} onClose={vi.fn()} />),
  };

  for (const [name, mount] of Object.entries(screens)) {
    it(`${name}: no raw keys, no ICU left, controls fit`, async () => {
      await act(async () => {
        await i18n.changeLanguage(lng);
      });
      const { container } = mount();
      const text = container.textContent ?? "";
      expect(text).not.toMatch(/\b[a-z]+\.[a-z_]+\.[a-z_]+\b/); // an untranslated key
      expect(text).not.toMatch(/\{[a-z_]+[,}]/); // an unformatted ICU variable
      const tooWide = controlTexts(container).filter((s) => columns(s) > CONTROL_BUDGET);
      expect(tooWide).toEqual([]);
    });
  }
});
