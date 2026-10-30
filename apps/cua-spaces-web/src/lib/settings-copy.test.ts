// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { lastChecked, plainStorage } from "./settings-copy";

describe("settings copy", () => {
  it("says last checked", () => {
    expect(lastChecked("Last check: 2 minutes ago")).toBe("Last checked 2 minutes ago");
  });

  it("calls S3-compatible storage your own storage", () => {
    expect(plainStorage("S3-compatible bucket")).toBe("Your own storage (S3)");
  });
});
