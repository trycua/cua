// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `relay.usage`: the relay's `GET /v1/usage` with the account's token, as the
// page's camelCase shape, checked against the web contract.
import { describe, expect, it, vi } from "vitest";
import { OP_SHAPES, SHAPE_DEFS } from "../../cua-spaces-web/src/bridge/contracts/shapes";
import { validate } from "../../cua-spaces-web/src/bridge/contracts/schema";
import type { BridgeContext } from "../src/bridge/context";
import { relayUrl, relayUsageMethods } from "../src/bridge/relay-usage";

const RELAY = {
  metered: true, period: "2026-10", plan: "free", team: { id: "acme", seats: 2 },
  live_sessions: 1, live_sessions_limit: 2, hours_used: 20.4, hours_limit: 20, my_hours_used: 3.4,
  soft_limit_reached: true, reasons: ["hours"], upgrade_url: "https://cua.ai/pricing",
};

function context(o: { signedIn?: boolean; relay?: string; env?: NodeJS.ProcessEnv } = {}) {
  const accessToken = vi.fn(async () => (o.signedIn === false ? null : "acct-token"));
  const ctx = {
    model: {
      servicesIn: true,
      identity: o.signedIn === false ? null : "ada@example.com",
      account: { accessToken },
      devices: { devices: () => (o.relay ? { relayUrl: () => o.relay } : null) },
    },
    env: o.env ?? {},
  } as unknown as BridgeContext;
  return { ctx, accessToken };
}

const answer = (status: number, body: unknown = null) =>
  vi.fn(async () => new Response(body === null ? null : JSON.stringify(body), { status })) as unknown as typeof fetch & ReturnType<typeof vi.fn>;

describe("relay.usage", () => {
  it("reads the allowance from the session's relay with the account's token", async () => {
    const { ctx } = context({ relay: "https://relay.example/" });
    const fetch = answer(200, RELAY);
    const usage = await relayUsageMethods(ctx, fetch)["relay.usage"]!({}, {});
    expect(fetch).toHaveBeenCalledWith("https://relay.example/v1/usage", expect.objectContaining({ headers: { authorization: "Bearer acct-token" } }));
    expect(usage).toEqual({
      metered: true, period: "2026-10", plan: "free", team: { id: "acme", seats: 2 },
      liveSessions: 1, liveSessionsLimit: 2, hoursUsed: 20.4, hoursLimit: 20, myHoursUsed: 3.4,
      softLimitReached: true, reasons: ["hours"], upgradeUrl: "https://cua.ai/pricing",
    });
    expect(validate(OP_SHAPES["relay.usage"], usage, SHAPE_DEFS)).toEqual([]);
    // An older relay's answer with fields missing takes the defaults.
    const bare = await relayUsageMethods(ctx, answer(200, { metered: true }))["relay.usage"]!({}, {});
    expect(validate(OP_SHAPES["relay.usage"], bare, SHAPE_DEFS)).toEqual([]);
  });

  it("is null signed out and from a relay that meters nothing; fails on other errors", async () => {
    const out = context({ signedIn: false });
    const fetch = answer(200, RELAY);
    expect(await relayUsageMethods(out.ctx, fetch)["relay.usage"]!({}, {})).toBeNull();
    expect(fetch).not.toHaveBeenCalled();
    const { ctx } = context();
    expect(await relayUsageMethods(ctx, answer(404))["relay.usage"]!({}, {})).toBeNull();
    await expect(relayUsageMethods(ctx, answer(500))["relay.usage"]!({}, {})).rejects.toMatchObject({ code: "failed" });
  });

  it("resolves the relay as the CLI does, and sends the token over https only (or to this machine)", () => {
    expect(relayUrl(context().ctx)).toBe("https://relay.cua.ai");
    expect(relayUrl(context({ env: { CUA_RELAY_URL: " https://r.example/ " } }).ctx)).toBe("https://r.example");
    expect(relayUrl(context({ relay: "http://127.0.0.1:8080", env: { CUA_RELAY_URL: "https://r.example" } }).ctx)).toBe("http://127.0.0.1:8080");
    expect(relayUrl(context({ env: { CUA_RELAY_URL: "http://relay.example" } }).ctx)).toBeNull();
  });
});
