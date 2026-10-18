// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { displayPairingCode, normalizePairingCode, pairingLinkRelay } from "../pairing-code";

// The same cases as `cua_host::pairing` (libs/cua/crates/cua-host/src/pairing.rs).
describe("pairing codes", () => {
  it("reads a code in any case, with or without dashes and spaces", () => {
    for (const typed of ["K7QM-4XPB-2RHD", "k7qm-4xpb-2rhd", "K7QM4XPB2RHD", " k7qm 4xpb 2rhd ", "cpc_K7QM4XPB2RHD"]) {
      expect(normalizePairingCode(typed)).toBe("K7QM4XPB2RHD");
    }
  });

  it("reads I and L as 1, and O as 0", () => {
    expect(normalizePairingCode("IL0O-IL0O-IL0O")).toBe("110011001100");
  });

  it("reads an add-machine link", () => {
    expect(normalizePairingCode("https://cua.ai/add/K7QM-4XPB-2RHD")).toBe("K7QM4XPB2RHD");
    expect(normalizePairingCode("https://cua.ai/add/K7QM-4XPB-2RHD?relay=https%3A%2F%2Frelay.example")).toBe("K7QM4XPB2RHD");
  });

  it("rejects what cannot be a code", () => {
    for (const typed of ["", "K7QM-4XPB", "K7QM-4XPB-2RHD-9", "K7QM-4XPB-2RH!", "U7QM-4XPB-2RHD", "https://cua.ai/add/"]) {
      expect(normalizePairingCode(typed)).toBeNull();
    }
  });

  it("shows a code as XXXX-XXXX-XXXX and leaves other input alone", () => {
    expect(displayPairingCode("k7qm4xpb2rhd")).toBe("K7QM-4XPB-2RHD");
    expect(displayPairingCode("nope")).toBe("nope");
  });

  it("finds the relay a link names", () => {
    expect(pairingLinkRelay("https://cua.ai/add/K7QM-4XPB-2RHD?relay=https%3A%2F%2Frelay.example")).toBe("https://relay.example");
    expect(pairingLinkRelay("https://cua.ai/add/K7QM-4XPB-2RHD")).toBeNull();
    expect(pairingLinkRelay("K7QM-4XPB-2RHD")).toBeNull();
  });
});
