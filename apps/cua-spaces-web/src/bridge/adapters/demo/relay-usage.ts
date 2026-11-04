// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The demo host's relay allowance: a free plan partway through the month
 * (`relayUsage` in its state; tests set it past the allowance). */

import type { RelayUsage } from "../../ops/relay-usage";
import type { DemoContext, DemoHandlers } from "./context";

export interface DemoRelayUsageState {
  relayUsage: RelayUsage | null;
}

export const demoRelayUsage = (): RelayUsage => ({
  metered: true,
  period: "2026-10",
  plan: "free",
  team: null,
  liveSessions: 1,
  liveSessionsLimit: 2,
  hoursUsed: 6.5,
  hoursLimit: 20,
  myHoursUsed: 6.5,
  softLimitReached: false,
  reasons: [],
  upgradeUrl: "https://cua.ai/pricing",
});

export const demoRelayUsageState = (): DemoRelayUsageState => ({ relayUsage: demoRelayUsage() });

export function demoRelayUsageHandlers({ state }: DemoContext): DemoHandlers<"relay.usage"> {
  return {
    // Signed out: no account, no allowance.
    "relay.usage": () => (state.identity && state.relayUsage ? { ...state.relayUsage, reasons: [...state.relayUsage.reasons] } : null),
  };
}

/** The allowance for `?demo=usage-<name>` (see `DemoOptions.usage`). */
export function demoRelayUsageNamed(name: "idle" | "warn" | "limit" | "pro" | "teams"): RelayUsage {
  const free = demoRelayUsage();
  switch (name) {
    case "idle":
      return { ...free, liveSessions: 0 };
    case "warn":
      return { ...free, liveSessions: 0, hoursUsed: 16.5, myHoursUsed: 16.5 };
    case "limit":
      return { ...free, hoursUsed: 20.4, myHoursUsed: 20.4, softLimitReached: true, reasons: ["hours"] };
    case "pro":
      return { ...free, plan: "pro", liveSessions: 0, liveSessionsLimit: 5, hoursUsed: 38, myHoursUsed: 38, hoursLimit: 200 };
    case "teams":
      return { ...free, plan: "team", team: { id: "acme", seats: 3 }, liveSessions: 0, liveSessionsLimit: 15, hoursUsed: 120, myHoursUsed: 41, hoursLimit: 600 };
  }
}
