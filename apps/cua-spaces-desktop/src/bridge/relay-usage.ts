// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The account's relay allowance this month (`relay.usage`; `cua usage`):
// the Spaces app export's `appRelayUsage` (the relay's `GET /v1/usage` on
// the session's relay, with the account's token), the same read as the
// SwiftUI host's. Null when signed out, or from a relay that meters nothing.
import type { BridgeContext } from "./context";
import type { Handlers } from "./host";
import { encode } from "./value";

export function relayUsageMethods(ctx: BridgeContext): Handlers {
  return {
    "relay.usage": async () => {
      const { model } = ctx;
      // Before the session is up, or signed out: nothing to read.
      if (!model.servicesIn || !model.identity || !model.account) return null;
      const devices = model.devices.devices();
      const token = await model.account.accessToken(false);
      if (!devices || !token) return null;
      const usage = await model.native.appRelayUsage(devices, token);
      return usage?.metered ? encode(usage) : null;
    },
  };
}
