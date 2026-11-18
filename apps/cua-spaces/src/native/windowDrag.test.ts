// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { screenToClient } from "./windowDrag";

describe("screenToClient", () => {
  it("maps a screen point into a window's client space", () => {
    // Window top-left at (100, 40) logical points; a drop at (260, 120).
    expect(screenToClient({ x: 260, y: 120 }, { x: 100, y: 40 })).toEqual({ x: 160, y: 80 });
  });

  it("is origin-relative", () => {
    expect(screenToClient({ x: 0, y: 0 }, { x: 0, y: 0 })).toEqual({ x: 0, y: 0 });
    expect(screenToClient({ x: 5, y: 5 }, { x: 10, y: 10 })).toEqual({ x: -5, y: -5 });
  });
});
