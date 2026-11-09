// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The English catalog files by path, for code that runs outside Vite. The
 * page finds them with `import.meta.glob`, which only Vite understands; the
 * parity specs run through Playwright's own loader and still import modules
 * that import `./index`. `index.test.ts` checks this list against the folder.
 */

import common from "./locales/en/common.json" with { type: "json" };
import computers from "./locales/en/computers.json" with { type: "json" };
import home from "./locales/en/home.json" with { type: "json" };
import settings from "./locales/en/settings.json" with { type: "json" };
import shell from "./locales/en/shell.json" with { type: "json" };
import spaces from "./locales/en/spaces.json" with { type: "json" };
import teleport from "./locales/en/teleport.json" with { type: "json" };

export const ENGLISH_FILES: Record<string, Record<string, string>> = {
  "./locales/en/common.json": common,
  "./locales/en/computers.json": computers,
  "./locales/en/home.json": home,
  "./locales/en/settings.json": settings,
  "./locales/en/shell.json": shell,
  "./locales/en/spaces.json": spaces,
  "./locales/en/teleport.json": teleport,
};
