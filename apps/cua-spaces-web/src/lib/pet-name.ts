// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { PET_ADJECTIVES, PET_ANIMALS } from "./pet-names.generated";

/**
 * A friendly default name (`brave-otter`), as the daemon makes for a Space created without one. The page names a
 * home before the daemon replies, so it carries the same lists (generated from the core's, with a drift test).
 * A taken name is retried with another pair; a short suffix is the last resort.
 */
export function petName(taken: (name: string) => boolean, random: () => number = Math.random): string {
  const pick = <T>(l: readonly T[]) => l[Math.floor(random() * l.length)]!;
  for (let i = 0; i < 64; i++) {
    const n = `${pick(PET_ADJECTIVES)}-${pick(PET_ANIMALS)}`;
    if (!taken(n)) return n;
  }
  for (;;) {
    const n = `${pick(PET_ADJECTIVES)}-${pick(PET_ANIMALS)}-${Math.floor(random() * 0x10000)
      .toString(16)
      .padStart(4, "0")}`;
    if (!taken(n)) return n;
  }
}
