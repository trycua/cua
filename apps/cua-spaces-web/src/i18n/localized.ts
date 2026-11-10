// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Source paths (relative to `src/`, files or directories) whose words are
 * all in the catalog. `no-hardcoded-strings.test.ts` fails on English left
 * in them. A PR that extracts a screen adds its paths here; the list grows
 * until it is `["."]`.
 */
export const LOCALIZED: readonly string[] = [
  "i18n",
  // Shell: sidebar, top bar, menus, command palette.
  "components/shell",
  "components/ui/menu.tsx",
  "routes/__root.tsx",
  "lib/commands.ts",
  // Home.
  "components/ask-first",
  "components/home",
  "lib/pool.ts",
  "routes/home",
  // Computers.
  "components/spaces",
  "components/computers",
  "components/machines",
  "components/add-computer-sheet.tsx",
  "components/this-machine.tsx",
  "components/this-mac-nudges.tsx",
  "routes/spaces",
  // Settings.
  "components/settings",
  "components/settings-group.tsx",
  "components/cli-hint.tsx",
  "routes/settings",
  "lib/settings-copy.ts",
];
