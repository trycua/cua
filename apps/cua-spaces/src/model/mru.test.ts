// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { FIXTURE_NOW, FIXTURE_SPACES } from "./fixtures";
import { ambientDots, countActive, sortByMru, touchSpace } from "./mru";

describe("sortByMru", () => {
  it("orders fixtures This Mac, Windows QA, Linux Build, Research", () => {
    expect(sortByMru(FIXTURE_SPACES).map((s) => s.name)).toEqual([
      "This Mac",
      "Windows QA",
      "Linux Build",
      "Research",
    ]);
  });

  it("does not mutate the input", () => {
    const input = [...FIXTURE_SPACES].reverse();
    const snapshot = input.map((s) => s.id);
    sortByMru(input);
    expect(input.map((s) => s.id)).toEqual(snapshot);
  });

  it("breaks timestamp ties by name", () => {
    const tied = FIXTURE_SPACES.map((s) => ({ ...s, lastUsedAt: 1 }));
    expect(sortByMru(tied).map((s) => s.name)).toEqual(["Linux Build", "Research", "This Mac", "Windows QA"]);
  });
});

describe("touchSpace", () => {
  it("moves the touched Space to the front after sorting", () => {
    const touched = sortByMru(touchSpace(FIXTURE_SPACES, "research", FIXTURE_NOW + 1000));
    expect(touched[0]?.id).toBe("research");
    expect(touched.slice(1).map((s) => s.id)).toEqual(["this-mac", "windows-qa", "linux-build"]);
  });

  it("ignores unknown ids", () => {
    expect(touchSpace(FIXTURE_SPACES, "nope", 1)).toEqual(FIXTURE_SPACES);
  });
});

describe("ambient summary", () => {
  it("counts running and approval as active", () => {
    expect(countActive(FIXTURE_SPACES)).toBe(2);
  });

  it("lights every dot for the fixture set", () => {
    expect(ambientDots(FIXTURE_SPACES)).toEqual({
      running: true,
      approval: true,
      suspended: true,
      agentActive: true,
    });
  });
});
