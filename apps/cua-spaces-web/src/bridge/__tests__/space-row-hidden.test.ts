// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { spaceToRow } from "../adapters/webkit";

const base = { id: "local:a", name: "a", os: "linux", status: "running", detail: "Running", lastUsedAt: 0, sdk: { features: [], spacesdVersion: "", reachable: true } };

describe("the host's hidden mark reaches the page's row", () => {
  // The macOS app's row carried `hidden` (label `cua.hidden`) but this mapping dropped it, so the home warming for Home's box
  // was listed in Computers as a Stopped Space and counted, though every list filters on it.
  it("keeps hidden on the row, and adds nothing to the others", () => {
    expect(spaceToRow({ ...base, id: "local:home-claude-code", hidden: true } as never).hidden).toBe(true);
    expect(spaceToRow(base as never)).not.toHaveProperty("hidden");
  });
});
