// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// An agent row that says "Waiting for a new Space to be ready" must end: a job whose create left no row, or that
// nobody finished making, turns into "Didn't start" with Try again (it sat there for 13 hours once).

import { describe, expect, it } from "vitest";

import { CREATE_MAX_MS, CREATE_ROW_GRACE_MS, stillCreating } from "@/hooks/use-agent-attempts";

const NOW = 1_000_000_000_000;
const task = (agoMs: number) => ({ createdMs: NOW - agoMs, pendingId: "pending:x" });

describe("stillCreating", () => {
  it("waits for a new job whose create row has not shown yet", () => {
    expect(stillCreating(task(10_000), false, NOW)).toBe(true);
    expect(stillCreating(task(CREATE_ROW_GRACE_MS), false, NOW)).toBe(true);
  });
  it("stops waiting for a job whose create left no row", () => {
    expect(stillCreating(task(CREATE_ROW_GRACE_MS + 1), false, NOW)).toBe(false);
    expect(stillCreating(task(13 * 3_600_000), false, NOW)).toBe(false);
  });
  it("keeps waiting while the create's row is there (a long first download), up to the bound", () => {
    expect(stillCreating(task(20 * 60_000), true, NOW)).toBe(true);
    expect(stillCreating(task(CREATE_MAX_MS + 1), true, NOW)).toBe(false);
  });
});
