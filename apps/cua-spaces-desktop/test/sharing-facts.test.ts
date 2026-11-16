// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The SDK's sharing facts as the core's row field (no native library needed).
import { describe, expect, it } from "vitest";
import { sharingFacts } from "../src/model/backend";

describe("sharing facts", () => {
  it("are absent for a Space that is not on the relay", () => {
    expect(sharingFacts(undefined)).toBeUndefined();
  });

  it("carry the owner, the role, the team and who has it, empty texts absent", () => {
    expect(
      sharingFacts({
        ownerId: "sam",
        ownerEmail: "sam@example.com",
        ownerName: "",
        role: "viewer",
        team: "org-qa",
        teamName: "QA team",
        editors: [],
        viewers: [],
        teams: [{ key: "org-qa", name: "QA team", role: "viewer" }],
      }),
    ).toEqual({
      ownerId: "sam",
      ownerEmail: "sam@example.com",
      ownerName: undefined,
      role: "viewer",
      team: "org-qa",
      teamName: "QA team",
      editors: [],
      viewers: [],
      teams: [{ key: "org-qa", name: "QA team", role: "viewer" }],
    });
  });
});
