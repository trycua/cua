// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { plural } from "./plural";

describe("plural", () => {
  it("uses the singular for exactly one", () => {
    expect(plural(1, "Space")).toBe("1 Space");
  });

  it("adds an s otherwise", () => {
    expect(plural(0, "machine")).toBe("0 machines");
    expect(plural(3, "Space")).toBe("3 Spaces");
  });

  it("takes an irregular plural", () => {
    expect(plural(2, "entry", "entries")).toBe("2 entries");
  });
});
