// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Source paths (relative to `src/`, files or directories) whose words are
 * all in the catalog. `no-hardcoded-strings.test.ts` fails on English left
 * in them. A PR that extracts a screen adds its paths here; the list grows
 * until it is `["."]`.
 */
export const LOCALIZED: readonly string[] = ["i18n", "components/settings/language-row.tsx"];
