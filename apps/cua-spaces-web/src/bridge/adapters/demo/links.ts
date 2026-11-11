// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The demo host's links: in memory, to a made-up relay address. A demo
 * with `notOnRelay` set refuses a new link the way a machine that was never
 * set up does. */

import { HostError } from "../../adapter";
import type { ShareLink, ShareSettings } from "../../ops/links";
import type { DemoContext, DemoHandlers } from "./context";

export interface DemoLinksState {
  links: ShareLink[];
  /** Refuse `links.create` as a machine not on the relay does. */
  notOnRelay: boolean;
  settings: ShareSettings;
  /** Where "auto" lands: through a share edge that asks for consent first (`?demo=shares-consent`), else Cua. */
  edgeFirst: boolean;
}

/** Made-up numbers for previews: a documentation address, an EC2 name and Cua's relay. */
const HOST_ADDRESS = "203.0.113.24";
const EDGE_HOST = "ec2-198-51-100-7.us-west-2.compute.amazonaws.com";
const GIB = 1024 ** 3;

export const SHARE_CONSENT_TEXT =
  "This creates a small server in your AWS account (about $7 a month plus $0.09 per GB players download). Remove it any time. Shares stop for the day after 2 GB (change it in Settings).";

export const defaultShareSettings = (): ShareSettings => ({
  landing: "auto",
  relayFallback: true,
  dailyCapMib: 2048,
  defaultAllow: [],
  approveAgentShares: true,
  alwaysAllowSpaces: [],
  edgeConsent: [],
  usedToday: 0,
  edgeCloud: null,
  edgeHost: null,
  edgeMonthlyUsd: null,
});

export type DemoSharesPreset = "consent" | "routes" | "settings" | "cap";

/** A TCP share as the daemon lists it, live on `route`. */
function liveShare(n: number, route: "host" | "edge" | "relay", target: string, nowSec: number, extra: Partial<ShareLink> = {}): ShareLink {
  const address = route === "host" ? `${HOST_ADDRESS}:${26_500 + n}` : route === "edge" ? `${EDGE_HOST}:${27_000 + n}` : `tcp.m.cua.sh:${26_000 + n}`;
  const connect = route === "relay" ? `mc-${(0x1a2b3c4d + n - 1).toString(16)}.m.cua.sh` : address;
  return {
    id: `share-${n.toString(16).padStart(12, "0")}`,
    url: `tcp://${address}`,
    name: null,
    access: "public",
    kind: "space_port",
    target,
    owner: "ada@example.com",
    createdAtUnix: nowSec,
    expiresAtUnix: nowSec + 86_400,
    tcpConnectWith: connect,
    tcpAddress: address,
    route,
    routeLine: route === "relay" ? "Through Cua" : "Through your AWS account",
    allow: [],
    waiting: null,
    waitingText: null,
    ...extra,
  };
}

/** The Space an agent asks from, in the previews. */
function waitingApproval(n: number, nowSec: number): ShareLink {
  return {
    id: `share-${n.toString(16).padStart(12, "0")}`,
    url: "",
    name: null,
    access: "public",
    kind: "space_port",
    target: "space:local:mc:25565",
    owner: "ada@example.com",
    createdAtUnix: nowSec,
    expiresAtUnix: null,
    tcpConnectWith: null,
    tcpAddress: null,
    route: null,
    routeLine: null,
    allow: [],
    waiting: "approval",
    waitingText: "An agent in local:mc wants to share port 25565 publicly. Anyone with the address can connect.",
  };
}

export function demoLinksState(now = Date.now(), preset?: DemoSharesPreset): DemoLinksState {
  const t = Math.floor(now / 1000);
  const state: DemoLinksState = { links: [], notOnRelay: false, settings: defaultShareSettings(), edgeFirst: false };
  if (preset === "consent") state.edgeFirst = true;
  if (preset === "routes") {
    state.settings = { ...state.settings, edgeCloud: "AWS", edgeHost: EDGE_HOST, edgeMonthlyUsd: 7, edgeConsent: ["AWS"] };
    const at = (address: string) => ({ tcpAddress: address, tcpConnectWith: address, url: `tcp://${address}` });
    state.links = [
      waitingApproval(4, t),
      liveShare(3, "relay", "space:local:mc:25565", t, { expiresAtUnix: t + 7 * 3600, tcpConnectWith: "mc-1a2b3c4d.m.cua.sh" }),
      liveShare(2, "edge", "space:local:survival:25565", t, { expiresAtUnix: t + 19 * 3600, ...at(`${EDGE_HOST}:27003`) }),
      liveShare(1, "host", "space:local:mc:25565", t, { expiresAtUnix: t + 23 * 3600, ...at(`${HOST_ADDRESS}:26500`) }),
    ];
  }
  if (preset === "settings" || preset === "cap") {
    const used = preset === "cap" ? Math.round(1.4 * GIB) : 380 * 1024 ** 2;
    state.settings = { ...state.settings, edgeCloud: "AWS", edgeHost: EDGE_HOST, edgeMonthlyUsd: 7, edgeConsent: ["AWS"], usedToday: used };
  }
  return state;
}

export const NOT_ON_RELAY = "this machine is not on your cua.ai relay, so a link would only open here. Run `cua host setup` once, then share again";

type LinksOpNames =
  | "links.list"
  | "links.create"
  | "links.stop"
  | "links.approve"
  | "links.deny"
  | "shares.settings.get"
  | "shares.settings.set"
  | "shares.edge.remove";

export function demoLinksHandlers({ state, now }: DemoContext): DemoHandlers<LinksOpNames> {
  const live = () => state.links.links.filter((l) => l.expiresAtUnix === null || l.expiresAtUnix * 1000 > now());
  const nowSec = () => Math.floor(now() / 1000);
  const nextId = () => state.links.links.length + 1 + state.links.links.reduce((m, l) => Math.max(m, Number.parseInt(l.id.slice(6), 16) || 0), 0);
  const find = (id: string) => {
    const l = state.links.links.find((x) => x.id === id);
    if (!l) throw new HostError(`no share ${id}`, "not_found");
    return l;
  };
  const replace = (l: ShareLink) => {
    state.links.links = state.links.links.map((x) => (x.id === l.id ? l : x));
    return { ...l };
  };
  /** Where an "auto" share lands. */
  const routeFor = (asked: string | undefined): "host" | "edge" | "relay" => {
    if (asked === "host" || asked === "edge" || asked === "relay") return asked;
    return state.links.edgeFirst || state.links.settings.edgeHost ? "edge" : "relay";
  };
  const goLive = (l: ShareLink, route: "host" | "edge" | "relay"): ShareLink => {
    if (route === "edge" && !state.links.settings.edgeHost) {
      state.links.settings = { ...state.links.settings, edgeCloud: "AWS", edgeHost: EDGE_HOST, edgeMonthlyUsd: 7, edgeConsent: ["AWS"] };
    }
    const n = Number.parseInt(l.id.slice(6), 16);
    return liveShare(n, route, l.target, nowSec(), { id: l.id, name: l.name, allow: l.allow ?? [] });
  };
  return {
    "links.list": () => ({ links: live().map((l) => ({ ...l })) }),
    "links.create": (a) => {
      if (state.links.notOnRelay) throw new HostError(NOT_ON_RELAY, "unsupported");
      if (a.tcp) {
        // A public TCP share (Labs): live at once, unless the first share through a share edge needs the person's yes.
        const n = nextId();
        const route = routeFor(a.route);
        const allow = a.allow ?? state.links.settings.defaultAllow;
        const draft = liveShare(n, route, a.target, nowSec(), { name: a.name ?? null, allow, expiresAtUnix: nowSec() + (a.ttlSeconds ?? 86_400) });
        if (a.service === "minecraft" && route === "relay") draft.tcpConnectWith = `mc-${(0x1a2b3c4d + n - 1).toString(16)}.m.cua.sh`;
        else if (route === "relay") draft.tcpConnectWith = draft.tcpAddress ?? null;
        if (route === "edge" && !state.links.settings.edgeHost && !a.consent) {
          const waiting: ShareLink = {
            ...draft,
            url: "",
            tcpConnectWith: null,
            tcpAddress: null,
            route: null,
            routeLine: null,
            expiresAtUnix: null,
            waiting: "consent",
            waitingText: SHARE_CONSENT_TEXT,
          };
          state.links.links = [waiting, ...state.links.links];
          return { ...waiting };
        }
        const made = goLive(draft, route);
        const link = { ...made, expiresAtUnix: draft.expiresAtUnix, allow };
        state.links.links = [link, ...state.links.links];
        return { ...link };
      }
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
        createdAtUnix: nowSec(),
        expiresAtUnix: ttl === null ? null : nowSec() + ttl,
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
    "links.approve": ({ id, always }) => {
      const l = find(id);
      if (!l.waiting) return { ...l };
      if (l.waiting === "approval" && always) {
        const space = l.target.split(":").slice(1, -1).join(":");
        const s = state.links.settings;
        if (!s.alwaysAllowSpaces.includes(space)) state.links.settings = { ...s, alwaysAllowSpaces: [...s.alwaysAllowSpaces, space] };
      }
      const route = routeFor(undefined);
      const made = goLive(l, route);
      return replace({ ...made, expiresAtUnix: nowSec() + 86_400 });
    },
    "links.deny": ({ id }) => {
      const l = find(id);
      if (!l.waiting) throw new HostError("that share is not waiting", "invalid");
      state.links.links = state.links.links.filter((x) => x.id !== id);
      return { denied: id };
    },
    "shares.settings.get": () => ({ ...state.links.settings }),
    "shares.settings.set": ({ settings }) => {
      const cur = state.links.settings;
      state.links.settings = {
        ...settings,
        // Read only: the daemon's.
        usedToday: cur.usedToday,
        edgeCloud: cur.edgeCloud,
        edgeHost: cur.edgeHost,
        edgeMonthlyUsd: cur.edgeMonthlyUsd,
      };
      return { ...state.links.settings };
    },
    "shares.edge.remove": () => {
      const ended = state.links.links.filter((l) => l.route === "edge").map((l) => l.id);
      state.links.links = state.links.links.filter((l) => !ended.includes(l.id));
      state.links.settings = { ...state.links.settings, edgeCloud: null, edgeHost: null, edgeMonthlyUsd: null, edgeConsent: [] };
      return { ended };
    },
  };
}
