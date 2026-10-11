// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The account's relay allowance this month (`relay.usage`; `cua usage`):
// the relay's `GET /v1/usage` with the account's token, as the page's
// camelCase `RelayUsage` (the web bridge's ops/relay-usage.ts). Null when
// signed out, or from a relay that meters nothing (404). Only this host
// reads it; the SwiftUI app does not yet.
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";

export const DEFAULT_RELAY_URL = "https://relay.cua.ai";

/** The relay the session's devices use, else `CUA_RELAY_URL`, else relay.cua.ai. Null: not one the token may go to (plain http off this machine). */
export function relayUrl(ctx: BridgeContext): string | null {
  let fromSession: string | undefined;
  try {
    fromSession = ctx.model.devices?.devices()?.relayUrl();
  } catch {
    fromSession = undefined;
  }
  const raw = (fromSession?.trim() || ctx.env.CUA_RELAY_URL?.trim() || DEFAULT_RELAY_URL).replace(/\/+$/, "");
  try {
    const url = new URL(raw.replace(/^wss:/, "https:").replace(/^ws:/, "http:"));
    const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
    if (url.protocol !== "https:" && !(url.protocol === "http:" && loopback)) return null;
    return url.toString().replace(/\/+$/, "");
  } catch {
    return null;
  }
}

const num = (v: unknown, fallback = 0) => (typeof v === "number" && Number.isFinite(v) ? v : fallback);
const maybeNum = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : null);
const str = (v: unknown) => (typeof v === "string" ? v : "");

/** The relay's snake_case answer as the page's `RelayUsage`; missing fields take their defaults. */
export function relayUsage(raw: unknown) {
  const r = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  const team = r.team && typeof r.team === "object" ? (r.team as Record<string, unknown>) : null;
  return {
    metered: r.metered === true,
    period: str(r.period),
    plan: str(r.plan),
    team: team ? { id: str(team.id), seats: num(team.seats) } : null,
    liveSessions: num(r.live_sessions),
    liveSessionsLimit: maybeNum(r.live_sessions_limit),
    hoursUsed: num(r.hours_used),
    hoursLimit: maybeNum(r.hours_limit),
    myHoursUsed: num(r.my_hours_used),
    softLimitReached: r.soft_limit_reached === true,
    reasons: Array.isArray(r.reasons) ? r.reasons.filter((x): x is string => typeof x === "string") : [],
    upgradeUrl: typeof r.upgrade_url === "string" && /^https:\/\//.test(r.upgrade_url) ? r.upgrade_url : null,
  };
}

export function relayUsageMethods(ctx: BridgeContext, fetchImpl: typeof fetch = (...a) => fetch(...a)): Handlers {
  return {
    "relay.usage": async () => {
      const { model } = ctx;
      // Before the session is up, or signed out: nothing to read.
      if (!model.servicesIn || !model.identity || !model.account) return null;
      const token = await model.account.accessToken(false);
      const base = relayUrl(ctx);
      if (!token || !base) return null;
      const resp = await fetchImpl(`${base}/v1/usage`, {
        headers: { authorization: `Bearer ${token}` },
        signal: AbortSignal.timeout(15_000),
      });
      if (resp.status === 404) return null;
      if (!resp.ok) throw Failure.failed(`The relay could not say your usage (HTTP ${resp.status})`);
      return relayUsage(await resp.json());
    },
  };
}
