// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { linkFor, needsYouIn } from "../src/notifications";
import type { SpaceList } from "../src/data/types";

describe("notifications", () => {
  it("deep links to the exact thread entry", () => {
    expect(linkFor({ spaceId: "relay:sample-checkout-api", seq: 7 })).toBe("/space/relay%3Asample-checkout-api?entry=7");
    expect(linkFor({ spaceId: "relay:a", seq: null })).toBe("/space/relay%3Aa");
  });

  it("posts each thing waiting on the reader once, by Space and entry", () => {
    const list = {
      spaces: [
        { id: "relay:a", name: "checkout-api", tone: "you", statusLine: "Waiting for you in checkout-api" },
        { id: "relay:b", name: "x", tone: "working", statusLine: "" },
      ],
    } as unknown as SpaceList;
    const needs = new Map([["relay:a", [{ seq: 7, kind: "approval" as const, text: "Claude Code wants to push" }]]]);
    expect(needsYouIn(list, needs)).toEqual([
      { key: "relay:a#7", spaceName: "checkout-api", payload: { spaceId: "relay:a", seq: 7, kind: "approval" }, text: "Claude Code wants to push" },
    ]);
  });
});
