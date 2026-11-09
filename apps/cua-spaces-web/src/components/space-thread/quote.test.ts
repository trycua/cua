// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { shownQuote } from "./thread-view";

describe("the reply quote line", () => {
  const q = { seq: 4 };
  it("a person's reply shows it unless it answers the message right above", () => {
    expect(shownQuote("person", q, 9)).toBe(q);
    expect(shownQuote("person", q, 4)).toBeNull();
  });
  it("an AI message and a Cua message never show it", () => {
    expect(shownQuote("agent", q, 9)).toBeNull();
    expect(shownQuote("system", q, 9)).toBeNull();
  });
});
