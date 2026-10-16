// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's sharing: who each Space is shared with, in memory. The
 * daemon's presence prompt is a short wait here; nothing reaches a relay.
 */

import type { ShareEntry } from "../../contracts/share";
import type { ShareHandlers } from "../../ops/share";

export function demoShares(): Map<string, ShareEntry[]> {
  return new Map([["local:design-review", [{ who: "grace@cua.ai", role: "editor", connected: true }]]]);
}

export function demoShareHandlers({ wait, stepMs }: { wait(ms: number): Promise<void>; stepMs: number }): ShareHandlers {
  const shares = demoShares();
  const list = (spaceId: string) => (shares.get(spaceId) ?? []).map((s) => ({ ...s }));
  return {
    "sharing.list": async ({ spaceId }) => list(spaceId),
    "sharing.share": async ({ spaceId, who, role }) => {
      // The daemon asks for presence before the relay hears of it.
      await wait(stepMs);
      const rest = (shares.get(spaceId) ?? []).filter((s) => s.who !== who);
      const was = shares.get(spaceId)?.find((s) => s.who === who);
      shares.set(spaceId, [...rest, { who, role, connected: was?.connected ?? false }]);
      return list(spaceId);
    },
    "sharing.unshare": async ({ spaceId, who }) => {
      shares.set(
        spaceId,
        (shares.get(spaceId) ?? []).filter((s) => s.who !== who),
      );
      return list(spaceId);
    },
  };
}
