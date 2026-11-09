// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A state file that is not what it should be (truncated by a crash or a full
// disk, or garbage) must not crash the app, and must not take what it held
// with it: the app starts from the defaults and the file stays beside it as
// `<name>.damaged`, until the next save writes a good one over the original.
// The core does the same for settings.json (`settings::load`).
import { copyFileSync, statSync } from "node:fs";

/** Files already copied this run (by path, size and time), so a file read on
 * every call is kept once, not on each read. */
const kept = new Set<string>();

/** Keeps `file` as `<file>.damaged` (the latest damaged one replaces an older
 * copy) and says so on stderr. Returns where it went, or null when it
 * could not be kept. Never throws. */
export function keepDamaged(file: string, why = "is damaged", warn: (message: string) => void = (m) => console.warn(m)): string | null {
  try {
    const st = statSync(file);
    const key = `${file}|${st.size}|${st.mtimeMs}`;
    const copy = `${file}.damaged`;
    if (kept.has(key)) return copy;
    // copyFileSync keeps the file's mode: a private file stays private.
    copyFileSync(file, copy);
    kept.add(key);
    warn(`${file} ${why}; starting from the defaults, the file is kept as ${copy}`);
    return copy;
  } catch {
    return null;
  }
}

/** Tests: forget what was kept. */
export function forgetKept(): void {
  kept.clear();
}
