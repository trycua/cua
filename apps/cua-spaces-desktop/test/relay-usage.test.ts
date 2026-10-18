// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `relay.usage`: the Spaces app export's `appRelayUsage` on the session's
// devices with the account's token, as the page's shape, checked against
// the web contract.
import { describe, expect, it, vi } from "vitest";
import { OP_SHAPES, SHAPE_DEFS } from "../../cua-spaces-web/src/bridge/contracts/shapes";
import { validate } from "../../cua-spaces-web/src/bridge/contracts/schema";
import type { BridgeContext } from "../src/bridge/context";
import { relayUsageMethods } from "../src/bridge/relay-usage";

/** As the generated bindings give the record: optionals are `undefined`. */
const NATIVE = {
  metered: true, period: "2026-10", plan: "free", team: { id: "acme", seats: 2 },
  liveSessions: 1, liveSessionsLimit: 2, hoursUsed: 20.4, hoursLimit: 20, myHoursUsed: 3.4,
  softLimitReached: true, reasons: ["hours"], upgradeUrl: "https://cua.ai/pricing",
};

function context(o: { signedIn?: boolean; devices?: boolean; usage?: unknown } = {}) {
  const accessToken = vi.fn(async () => (o.signedIn === false ? null : "acct-token"));
  const session = { relayUrl: () => "https://relay.example" };
  const appRelayUsage = vi.fn(async () => ("usage" in o ? o.usage : NATIVE));
  const ctx = {
    model: {
      servicesIn: true,
      identity: o.signedIn === false ? null : "ada@example.com",
      account: { accessToken },
      devices: { devices: () => (o.devices === false ? null : session) },
      native: { appRelayUsage },
    },
  } as unknown as BridgeContext;
  return { ctx, appRelayUsage, session };
}

const read = (ctx: BridgeContext) => relayUsageMethods(ctx)["relay.usage"]!({}, {});

describe("relay.usage", () => {
  it("reads the allowance through the app export with the session's devices and the account's token", async () => {
    const { ctx, appRelayUsage, session } = context();
    const usage = await read(ctx);
    expect(appRelayUsage).toHaveBeenCalledWith(session, "acct-token");
    expect(usage).toEqual(NATIVE);
    expect(validate(OP_SHAPES["relay.usage"], usage, SHAPE_DEFS)).toEqual([]);
    // No team, no limits: the record's missing optionals reach the page as null.
    const bare = await read(context({ usage: { ...NATIVE, team: undefined, liveSessionsLimit: undefined, hoursLimit: undefined, upgradeUrl: undefined } }).ctx);
    expect(bare).toMatchObject({ team: null, liveSessionsLimit: null, hoursLimit: null, upgradeUrl: null });
    expect(validate(OP_SHAPES["relay.usage"], bare, SHAPE_DEFS)).toEqual([]);
  });

  it("is null signed out, without devices, and from a relay that meters nothing", async () => {
    const out = context({ signedIn: false });
    expect(await read(out.ctx)).toBeNull();
    expect(out.appRelayUsage).not.toHaveBeenCalled();
    expect(await read(context({ devices: false }).ctx)).toBeNull();
    expect(await read(context({ usage: undefined }).ctx)).toBeNull();
    expect(await read(context({ usage: { ...NATIVE, metered: false } }).ctx)).toBeNull();
  });
});
