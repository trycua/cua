// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `sharing.removals` and `sharing.ackRemovals`: the Spaces app export's
// `appShareRemovals` and `appAckShareRemovals` with the account's token (the
// owner's one-line note that sharing is for teams now), as the page's shape,
// checked against the web contract.
import { describe, expect, it, vi } from "vitest";
import { OP_SHAPES, SHAPE_DEFS } from "../../cua-spaces-web/src/bridge/contracts/shapes";
import { validate } from "../../cua-spaces-web/src/bridge/contracts/schema";
import type { BridgeContext } from "../src/bridge/context";
import { sharingMethods } from "../src/bridge/sharing";

const REMOVALS = [
  { who: "alex@x.test", machineName: "Design" },
  { who: "sam@x.test", machineName: "Design" },
];

function context(o: { signedIn?: boolean } = {}) {
  const accessToken = vi.fn(async () => "acct-token");
  const appShareRemovals = vi.fn(async (_token: string) => REMOVALS);
  const appAckShareRemovals = vi.fn(async (_token: string) => {});
  const ctx = {
    model: {
      servicesIn: true,
      identity: o.signedIn === false ? null : "ada@example.com",
      account: { accessToken },
      native: { appShareRemovals, appAckShareRemovals },
    },
  } as unknown as BridgeContext;
  return { ctx, appShareRemovals, appAckShareRemovals };
}

const handlers = (ctx: BridgeContext) => sharingMethods(ctx);

describe("sharing.removals and sharing.ackRemovals", () => {
  it("reads who was removed with the account's token, in the page's shape", async () => {
    const { ctx, appShareRemovals } = context();
    const out = await handlers(ctx)["sharing.removals"]!({}, {});
    expect(appShareRemovals).toHaveBeenCalledWith("acct-token");
    expect(out).toEqual(REMOVALS);
    expect(validate(OP_SHAPES["sharing.removals"], out, SHAPE_DEFS)).toEqual([]);
  });

  it("acknowledges with the account's token and answers null", async () => {
    const { ctx, appAckShareRemovals } = context();
    expect(await handlers(ctx)["sharing.ackRemovals"]!({}, {})).toBeNull();
    expect(appAckShareRemovals).toHaveBeenCalledWith("acct-token");
  });

  it("reads nothing and acknowledges nothing when signed out", async () => {
    const out = context({ signedIn: false });
    expect(await handlers(out.ctx)["sharing.removals"]!({}, {})).toEqual([]);
    expect(await handlers(out.ctx)["sharing.ackRemovals"]!({}, {})).toBeNull();
    expect(out.appShareRemovals).not.toHaveBeenCalled();
    expect(out.appAckShareRemovals).not.toHaveBeenCalled();
  });
});
