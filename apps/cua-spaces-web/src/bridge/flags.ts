// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The web UI has no feature flags now. Each one became the default and was
 * deleted with its off path:
 *
 * - `spaces_redesign`: the Spaces page redesign.
 * - `spaces_tree` (CUA-3413): your computers and the Spaces on them in one
 *   list (CUA-3551).
 * - `spaces_ask_first_home` (CUA-2346): Home asks what you want done and opens
 *   the app.
 * - `spaces_new_onboarding` (CUA-2851, CUA-3318): the first run that ends on
 *   that Home.
 *
 * A device can still hold an override of one (`cua.flag.<name>` in
 * `localStorage`, from Settings → Advanced → Labs or a tester). It is cleared
 * once when this module loads, so a stale "0" can't matter if a flag of the
 * same name ever comes back.
 *
 * Labs experiments are not flags: the app core owns them (`useExperiments`).
 */
export const RETIRED_FLAGS = ["spaces_redesign", "spaces_tree", "spaces_ask_first_home", "spaces_new_onboarding"] as const;

/** Clears the stored override of every retired flag. Runs once when this module loads. */
export function clearRetiredFlagOverrides(): void {
  try {
    for (const name of RETIRED_FLAGS) globalThis.localStorage?.removeItem(`cua.flag.${name}`);
  } catch {
    // No storage: nothing to clear.
  }
}

clearRetiredFlagOverrides();
