// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Share a Space (`sharing.list`, `sharing.share`, `sharing.unshare`) and the
// owner's note that sharing is for teams now (`sharing.removals`,
// `sharing.ackRemovals`: the Spaces app export's `appShareRemovals` and
// `appAckShareRemovals` with the account's token, as the SwiftUI host reads
// them; empty when signed out or from a server that predates it). The
// SDK's `Space.shares`, `share` and `unshare`, as the SwiftUI host calls
// them for its Share sheet. Who it is shared with comes back as the core's
// share rows; the sheet's words, the experiment gate and the usage events
// are the page's (`bridge/share.ts`). Sharing asks for presence in the
// daemon before anything reaches the relay.
import { string } from "./args";
import type { BridgeContext } from "./context";
import type { Handlers } from "./host";
import { encode } from "./value";

export function sharingMethods({ model }: BridgeContext): Handlers {
  const token = async () => (model.servicesIn && model.identity && model.account ? model.account.accessToken(false) : null);
  return {
    "sharing.list": async (args) => encode(await model.backend.shares(string(args, "spaceId"))),
    "sharing.share": async (args) => encode(await model.backend.share(string(args, "spaceId"), string(args, "who"), string(args, "role"))),
    "sharing.unshare": async (args) => encode(await model.backend.unshare(string(args, "spaceId"), string(args, "who"))),
    "sharing.removals": async () => {
      const t = await token();
      return t ? encode(await model.native.appShareRemovals(t)) : [];
    },
    "sharing.ackRemovals": async () => {
      const t = await token();
      if (t) await model.native.appAckShareRemovals(t);
      return null;
    },
  };
}
