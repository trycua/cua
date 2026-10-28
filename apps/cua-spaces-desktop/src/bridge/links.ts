// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Links to things you run (`links.list`, `links.create`, `links.stop`): the
// SDK's `Cua.shares`, `share`, `stop_share`, as the SwiftUI host calls them.
// The cua daemon owns the shares; a machine that is not on the relay refuses
// `links.create` in the daemon's words, which the page shows with the fix.
import { count, optionalString, string } from "./args";
import type { BridgeContext } from "./context";
import type { Handlers } from "./host";
import { encode } from "./value";

export function linksMethods({ model }: BridgeContext): Handlers {
  return {
    "links.list": async () => ({ links: encode(await model.backend.links()) }),
    "links.create": async (args) =>
      encode(await model.backend.createLink(string(args, "target"), optionalString(args, "access"), count(args, "ttlSeconds"), optionalString(args, "name"))),
    "links.stop": async (args) => ({ stopped: await model.backend.stopLink(string(args, "id")) }),
  };
}
