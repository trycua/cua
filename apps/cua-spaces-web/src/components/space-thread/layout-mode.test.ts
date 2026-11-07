// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { beforeEach, describe, expect, it } from "vitest";

import { savedStreamHidden } from "./layout-mode";

beforeEach(() => localStorage.clear());

describe("savedStreamHidden", () => {
  it("shows the stream of an ordinary Space and folds a home's until the person chooses", () => {
    expect(savedStreamHidden("a")).toBe(false);
    expect(savedStreamHidden("a", true)).toBe(true);
  });

  it("keeps the person's choice over the default, either way, per Space", () => {
    localStorage.setItem("cua-spaces:stream-hidden:home", "0");
    localStorage.setItem("cua-spaces:stream-hidden:other", "1");
    expect(savedStreamHidden("home", true)).toBe(false);
    expect(savedStreamHidden("other", false)).toBe(true);
    expect(savedStreamHidden("third", true)).toBe(true);
  });
});
