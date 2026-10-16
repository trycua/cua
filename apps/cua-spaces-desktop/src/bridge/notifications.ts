// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Notifications (`notifications.*`): the daemon's feed (the SwiftUI host's
// `PersistentModel` feed, `model/notifications.ts`), read again on each
// visit; the poll that posts system notifications runs from `startHost`.
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { hostParts } from "./host-parts";
import { encode } from "./value";

export function notificationsMethods(ctx: BridgeContext): Handlers {
  const feed = () => {
    if (!hostParts(ctx).tool()) throw Failure.unsupported("Notifications need the cua daemon");
    return hostParts(ctx).notifications;
  };
  return {
    "notifications.list": async () => {
      const n = feed();
      await n.poll();
      // The core's entries, each with the Space it is about (Open Space).
      return n.feed.map((e) => ({ ...(encode(e) as Record<string, unknown>), space: n.spaceOf(e.id) }));
    },
    "notifications.markAllRead": async () => {
      await feed().markAllRead();
      return null;
    },
    "notifications.markRead": async (args) => {
      const n = feed();
      const ids = args.ids;
      if (!Array.isArray(ids) || !ids.every((id) => typeof id === "string")) throw Failure.badArgs("ids: array of strings");
      await n.markRead(ids);
      return null;
    },
  };
}
