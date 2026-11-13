// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { namesOf } from "./spaces-tree-page";

describe("namesOf (the delete dialog names the child Spaces)", () => {
  it("reads as a list", () => {
    expect(namesOf(["kid-a"])).toBe("kid-a");
    expect(namesOf(["kid-a", "kid-b"])).toBe("kid-a and kid-b");
    expect(namesOf(["a", "b", "c"])).toBe("a, b and c");
    expect(namesOf(["a", "b", "c", "d", "e", "f"])).toBe("a, b, c and 3 more");
    expect(namesOf([])).toBe("");
  });
});
