// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it, vi } from "vitest";

const setUiLocale = vi.fn();
vi.mock("../native/telemetry", () => ({ telemetryBridge: () => ({ setUiLocale }) }));

import { reportUiLocale } from "./reportLocale";

describe("reportUiLocale", () => {
  it("sends the locale in use to telemetry", () => {
    reportUiLocale();
    expect(setUiLocale).toHaveBeenCalledWith("en");
  });
});
