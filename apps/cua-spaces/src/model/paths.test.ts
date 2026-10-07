// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { displayPath, displayPaths } from "./paths";

describe("displayPath", () => {
  it("shows paths under home as ~/...", () => {
    expect(displayPath("/Users/ada/Projects/demo", "/Users/ada")).toBe("~/Projects/demo");
    expect(displayPath("/Users/ada", "/Users/ada/")).toBe("~");
  });
  it("leaves other paths and look-alike prefixes alone", () => {
    expect(displayPath("/Users/adam/x", "/Users/ada")).toBe("/Users/adam/x");
    expect(displayPath("/opt/x", "/Users/ada")).toBe("/opt/x");
    expect(displayPath("/Users/ada/x", null)).toBe("/Users/ada/x");
  });
  it("rewrites the home folder inside free text", () => {
    expect(displayPaths("1 file, from /Users/ada/Projects/demo to /home/cua", "/Users/ada")).toBe(
      "1 file, from ~/Projects/demo to /home/cua",
    );
    expect(displayPaths("/Users/adam/x", "/Users/ada")).toBe("/Users/adam/x");
  });
});
