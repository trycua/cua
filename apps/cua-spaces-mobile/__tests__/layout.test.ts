// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { layoutFor } from "../src/layout/layout";

describe("layoutFor (by window width, never by device)", () => {
  it.each([
    ["iPhone 402x874", 402, 874, "phone"],
    ["iPhone landscape 874x402 still reads as a phone-width window only under 600", 590, 402, "phone"],
    ["iPhone Duo folded outer 466x678", 466, 678, "phone"],
    ["iPhone Duo opened out 890x626", 890, 626, "duo"],
    ["iPad portrait 1032x1376", 1032, 1376, "split"],
    ["iPad Split View half 678x1024", 678, 1024, "split"],
    ["iPad landscape 1376x1032", 1376, 1032, "triple"],
    ["iPad landscape 11 inch 1194x834", 1194, 834, "triple"],
    ["a 700 wide landscape window", 700, 500, "duo"],
  ])("%s", (_name, w, h, expected) => {
    expect(layoutFor(w as number, h as number)).toBe(expected);
  });
});
