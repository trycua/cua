// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { readExperiments } from "../ops/volume";

describe("stored experiments", () => {
  it("reads an experiment as its default unless stored (Secure Spaces, Child Spaces, Ask first and Warm start on), a stored off stays off, and keys of switches that are gone are ignored", () => {
    expect(readExperiments(undefined)).toEqual({ cuaVolume: false, secureSpaces: true, childSpaces: true, askFirst: true, ciRunners: false, tcpShares: false, mobile: false, warmPools: true });
    expect(readExperiments(JSON.stringify({ cuaVolume: true }))).toMatchObject({ cuaVolume: true });
    expect(readExperiments(JSON.stringify({ warmPools: false, askFirst: false }))).toMatchObject({ warmPools: false, askFirst: false, secureSpaces: true });
    expect(readExperiments(JSON.stringify({ sharing: false, yourCloud: false, webUi: true }))).toEqual(readExperiments(undefined));
  });
});
