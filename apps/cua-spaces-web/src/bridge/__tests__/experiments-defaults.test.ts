// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { readExperiments } from "../ops/volume";

describe("stored experiments", () => {
  it("reads sharing as on unless it is stored off, and the rest as off", () => {
    expect(readExperiments(undefined)).toEqual({ cuaVolume: false, yourCloud: false, sharing: true, webUi: false });
    expect(readExperiments(JSON.stringify({ cuaVolume: true }))).toMatchObject({ cuaVolume: true, sharing: true });
    expect(readExperiments(JSON.stringify({ sharing: false }))).toMatchObject({ sharing: false });
  });
});
