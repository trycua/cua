// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { runningFor } from "../src/features/thread/running";

describe("runningFor", () => {
  it("counts seconds, then minutes", () => {
    expect(runningFor(1_000, 13_000)).toBe("12s");
    expect(runningFor(1_000, 66_000)).toBe("1m 05s");
    expect(runningFor(5_000, 1_000)).toBe("0s");
  });
});
