// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { readExperiments } from "../ops/volume";

describe("stored experiments", () => {
  it("reads an experiment as off unless stored, and ignores keys of switches that are gone (including the four that are always on now)", () => {
    const off = { cuaVolume: false, ciRunners: false, tcpShares: false, mobile: false };
    expect(readExperiments(undefined)).toEqual(off);
    expect(readExperiments(JSON.stringify({ cuaVolume: true }))).toMatchObject({ cuaVolume: true });
    expect(readExperiments(JSON.stringify({ warmPools: false, askFirst: false, secureSpaces: false, childSpaces: false }))).toEqual(off);
    expect(readExperiments(JSON.stringify({ sharing: false, yourCloud: false, webUi: true }))).toEqual(off);
  });
});
