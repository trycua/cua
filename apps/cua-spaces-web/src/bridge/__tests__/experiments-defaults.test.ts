// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { readExperiments } from "../ops/volume";

describe("stored experiments", () => {
  it("reads every experiment as off unless stored on, and ignores keys of switches that are gone", () => {
    expect(readExperiments(undefined)).toEqual({ cuaVolume: false, secureSpaces: false, childSpaces: false, askFirst: false, ciRunners: false, tcpShares: false, mobile: false, warmPools: false });
    expect(readExperiments(JSON.stringify({ cuaVolume: true }))).toMatchObject({ cuaVolume: true });
    expect(readExperiments(JSON.stringify({ sharing: false, yourCloud: false, webUi: true }))).toEqual(readExperiments(undefined));
  });
});
