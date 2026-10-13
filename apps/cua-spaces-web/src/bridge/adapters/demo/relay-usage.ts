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
