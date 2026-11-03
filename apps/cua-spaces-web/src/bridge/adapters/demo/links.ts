// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The demo host's links: in memory, to a made-up relay address. A demo
 * with `notOnRelay` set refuses a new link the way a machine that was never
 * set up does. */

import { HostError } from "../../adapter";
import type { ShareLink } from "../../ops/links";
import type { DemoContext, DemoHandlers } from "./context";

export interface DemoLinksState {
  links: ShareLink[];
  /** Refuse `links.create` as a machine not on the relay does. */
  notOnRelay: boolean;
}

export const demoLinksState = (): DemoLinksState => ({ links: [], notOnRelay: false });

export const NOT_ON_RELAY = "this machine is not on your cua.ai relay, so a link would only open here. Run `cua host setup` once, then share again";

export function demoLinksHandlers({ state, now }: DemoContext): DemoHandlers<"links.list" | "links.create" | "links.stop"> {
  const live = () => state.links.links.filter((l) => l.expiresAtUnix === null || l.expiresAtUnix * 1000 > now());
  return {
    "links.list": () => ({ links: live().map((l) => ({ ...l })) }),
    "links.create": (a) => {
      if (state.links.notOnRelay) throw new HostError(NOT_ON_RELAY, "unsupported");
      const access = a.access ?? "private";
      const id = `share-${(state.links.links.length + 1).toString(16).padStart(12, "0")}`;
      const kind = a.target.startsWith("space:") ? "space_port" : /^\d+$/.test(a.target) ? "port" : "file";
      const ttl = a.ttlSeconds ?? (access === "public" ? 3600 : null);
      const link: ShareLink = {
        id,
        url: access === "public" ? `https://p${id}.m.relay.example/` : `https://s${id}.m.relay.example/`,
        name: a.name ?? null,
        access,
        kind,
        target: kind === "port" ? `port ${a.target}` : a.target,
        owner: "ada@example.com",
        createdAtUnix: Math.floor(now() / 1000),
        expiresAtUnix: ttl === null ? null : Math.floor(now() / 1000) + ttl,
      };
      state.links.links = [link, ...state.links.links];
      return { ...link };
    },
    "links.stop": ({ id }) => {
      const gone = state.links.links.filter((l) => id === "all" || l.id === id).map((l) => l.id);
      if (gone.length === 0 && id !== "all") throw new HostError(`no share ${id}`, "not_found");
      state.links.links = state.links.links.filter((l) => !gone.includes(l.id));
      return { stopped: gone };
    },
  };
}
