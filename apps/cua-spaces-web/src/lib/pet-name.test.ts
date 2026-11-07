// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The friendly default names: the same lists as the core's (the core test fails when they drift).

import { describe, expect, it } from "vitest";

import { petName } from "./pet-name";
import { PET_ADJECTIVES, PET_ANIMALS } from "./pet-names.generated";

describe("petName", () => {
  it("is a lowercase adjective-animal, DNS-safe", () => {
    for (let i = 0; i < 200; i++) expect(petName(() => false)).toMatch(/^[a-z]+-[a-z]+$/);
  });

  it("retries another pair when taken, with a suffix only as the last resort", () => {
    const seen: string[] = [];
    const n = petName((c) => {
      seen.push(c);
      return seen.length === 1;
    });
    expect(n).not.toBe(seen[0]);
    expect(n.split("-")).toHaveLength(2);
    expect(petName((c) => c.split("-").length === 2).split("-")).toHaveLength(3);
  });

  it("lists are long enough and have no duplicates", () => {
    for (const l of [PET_ADJECTIVES, PET_ANIMALS]) {
      expect(l.length).toBeGreaterThanOrEqual(100);
      expect(new Set(l).size).toBe(l.length);
      for (const w of l) expect(w).toMatch(/^[a-z]+$/);
    }
  });
});
