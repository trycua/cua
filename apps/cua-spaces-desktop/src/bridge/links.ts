// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Links to things you run (`links.list`, `links.create`, `links.stop`): the
// SDK's `Cua.shares`, `share`, `stop_share`, as the SwiftUI host calls them.
// The cua daemon owns the shares; a machine that is not on the relay refuses
// `links.create` in the daemon's words, which the page shows with the fix.
import { bool, count, object, optionalString, string, strings } from "./args";
import type { BridgeContext } from "./context";
import type { Handlers } from "./host";
import { encode } from "./value";

/** A backend method, or the refusal of a host that has none. */
function need<A extends unknown[], R>(f: ((...a: A) => R) | undefined): (...a: A) => R {
  if (!f) throw new Error("Public TCP shares are not available here");
  return f;
}

export function linksMethods({ model }: BridgeContext): Handlers {
  return {
    "links.list": async () => ({ links: encode(await model.backend.links()) }),
    "links.create": async (args) => {
      // A public TCP share (Labs): a TCP service in a Space at a public address (agents and the CLI ask for them).
      if (args.tcp === true) {
        const create = model.backend.createTcpLink?.bind(model.backend);
        if (!create) throw new Error("Public TCP shares are not available here");
        const allow = Array.isArray(args.allow) ? strings(args, "allow") : null;
        return encode(
          await create(string(args, "target"), optionalString(args, "service"), count(args, "ttlSeconds"), optionalString(args, "name"), {
            route: optionalString(args, "route"),
            allow,
            checkHost: args.checkHost === true,
            consent: args.consent === true,
          }),
        );
      }
      return encode(
        await model.backend.createLink(string(args, "target"), optionalString(args, "access"), count(args, "ttlSeconds"), optionalString(args, "name")),
      );
    },
    "links.stop": async (args) => ({ stopped: await model.backend.stopLink(string(args, "id")) }),
    // A waiting public TCP share (an agent asked, or the first through a share edge): the person's answer.
    "links.approve": async (args) => encode(await need(model.backend.approveLink?.bind(model.backend))(string(args, "id"), args.always === true)),
    "links.deny": async (args) => {
      const id = string(args, "id");
      await need(model.backend.denyLink?.bind(model.backend))(id);
      return { denied: id };
    },
    "shares.settings.get": async () => encode(await need(model.backend.shareSettings?.bind(model.backend))()),
    "shares.settings.set": async (args) => {
      const s = object(args, "settings");
      return encode(
        await need(model.backend.setShareSettings?.bind(model.backend))({
          landing: s.landing === "only_my_cloud" || s.landing === "allow_relay" ? s.landing : "auto",
          relayFallback: bool(s, "relayFallback"),
          dailyCapMib: count(s, "dailyCapMib") ?? 0,
          defaultAllow: strings(s, "defaultAllow"),
          approveAgentShares: bool(s, "approveAgentShares"),
          alwaysAllowSpaces: strings(s, "alwaysAllowSpaces"),
          edgeConsent: strings(s, "edgeConsent"),
          // Read only: the daemon's.
          usedToday: 0,
          edgeCloud: null,
          edgeHost: null,
          edgeMonthlyUsd: null,
        }),
      );
    },
    "shares.edge.remove": async () => ({ ended: await need(model.backend.removeShareEdge?.bind(model.backend))() }),
  };
}
