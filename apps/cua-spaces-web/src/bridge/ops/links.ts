// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Links to things you run (`cua share`): a Space's port or a file on this
 * machine, as a link that opens on your other signed-in devices. The cua
 * daemon owns the shares, so a link outlives the app.
 *
 * | Operation | Electron | SwiftUI | Tauri |
 * |---|---|---|---|
 * | `links.list` | `Cua.shares` | `Cua.shares` | unsupported |
 * | `links.create {target, access?, ttlSeconds?, name?}` | `Cua.share` | `Cua.share` | unsupported |
 * | `links.create {target, tcp: true, service?, ttlSeconds?, name?}` (Labs) | `Cua.share_tcp` | `Cua.share_tcp` | unsupported |
 * | `links.create {target, tcp: true, route?, allow?, checkHost?, consent?}` | the same, with those options | the same | unsupported |
 * | `links.approve {id, always?}` (a waiting TCP share goes live) | `Cua.approve_share` | the same | unsupported |
 * | `links.deny {id}` | `Cua.deny_share` | the same | unsupported |
 * | `links.stop {id}` (`"all"` stops every link) | `Cua.stop_share`, `stop_all_shares` | the same | unsupported |
 * | `shares.settings.get`, `shares.settings.set` | `Cua.share_settings`, `set_share_settings` | the same | unsupported |
 * | `shares.edge.remove` | `Cua.remove_share_edge` | the same | unsupported |
 *
 * A machine that is not on the relay refuses `links.create` with the
 * daemon's words ("... Run `cua host setup`"); the page adds the fix.
 */

import { UnsupportedOperationError, type BridgeMode } from "../adapter";
import type { OpCoverage } from "../coverage";

export type LinkAccess = "private" | "team" | "public";

/** `cua_sdk::Share`. */
export interface ShareLink {
  id: string;
  url: string;
  name: string | null;
  access: LinkAccess;
  /** `port`, `space_port`, `file` or `dir`. */
  kind: string;
  /** `port 3000`, `space:dev:3000`, a path. */
  target: string;
  owner: string;
  createdAtUnix: number | null;
  /** Null: until stopped. */
  expiresAtUnix: number | null;
  /** A public TCP share (Labs): what people connect to, a name or host:port. Absent or null: an HTTP link. */
  tcpConnectWith?: string | null;
  /** A public TCP share: host:port on the relay. */
  tcpAddress?: string | null;
  /** A public TCP share's route: `host` (the Space's own machine in your cloud), `edge` (a share edge in your linked cloud) or `relay` (Cua). */
  route?: string | null;
  /** The route in plain words: "Through your AWS account", "Through Cua". */
  routeLine?: string | null;
  /** Addresses (CIDRs) that may connect. Empty or absent: anyone. */
  allow?: string[];
  /** Set while the share is not live (no address yet): `approval` (an agent asked) or `consent` (the first share through a new share edge). */
  waiting?: "approval" | "consent" | null;
  /** What the person is asked, in plain words. */
  waitingText?: string | null;
}

/** Where public TCP shares land. */
export type ShareLanding = "auto" | "only_my_cloud" | "allow_relay";

/** `cua_sdk::ShareSettings`. The last four fields are read only. */
export interface ShareSettings {
  landing: ShareLanding;
  relayFallback: boolean;
  /** MiB a day; 0: no cap. */
  dailyCapMib: number;
  /** CIDRs a new share starts with; empty: anyone. */
  defaultAllow: string[];
  /** Ask before an agent's share goes public. */
  approveAgentShares: boolean;
  alwaysAllowSpaces: string[];
  edgeConsent: string[];
  /** Bytes moved today. */
  usedToday: number;
  /** The share edge's cloud name ("AWS"), when there is one. */
  edgeCloud: string | null;
  edgeHost: string | null;
  edgeMonthlyUsd: number | null;
}

export interface LinkCreate {
  /** A port, `space:NAME:PORT`, or an absolute path on this machine. */
  target: string;
  access?: LinkAccess;
  ttlSeconds?: number;
  name?: string;
  /** A public TCP share (Labs: Public TCP shares) of `space:NAME:PORT` instead of a link; always public. */
  tcp?: boolean;
  /** With `tcp`: the game (`minecraft`), for a name people can type. */
  service?: string;
  /** With `tcp`: `auto` (default), `host`, `edge` or `relay`. */
  route?: string;
  /** With `tcp`: only these addresses (CIDRs) may connect. Absent or empty: anyone (or the default in Settings). */
  allow?: string[];
  /** With `tcp`: drop connections that did not dial the address (Minecraft). */
  checkHost?: boolean;
  /** With `tcp`: agree to a share edge's cost now instead of being asked. */
  consent?: boolean;
}

export interface LinksOps {
  "links.list": { args: Record<string, never>; result: { links: ShareLink[] } };
  "links.create": { args: LinkCreate; result: ShareLink };
  "links.stop": { args: { id: string }; result: { stopped: string[] } };
  /** A waiting share goes live; `always`: its Space may share without asking from now on. */
  "links.approve": { args: { id: string; always?: boolean }; result: ShareLink };
  "links.deny": { args: { id: string }; result: { denied: string } };
  "shares.settings.get": { args: Record<string, never>; result: ShareSettings };
  /** Saves the settings (the read-only fields are ignored) and answers with the saved ones. */
  "shares.settings.set": { args: { settings: ShareSettings }; result: ShareSettings };
  /** Removes the share edge from your cloud; the ids of the shares that ended with it. */
  "shares.edge.remove": { args: Record<string, never>; result: { ended: string[] } };
}

export const LINKS_OPERATIONS = [
  "links.list",
  "links.create",
  "links.stop",
  "links.approve",
  "links.deny",
  "shares.settings.get",
  "shares.settings.set",
  "shares.edge.remove",
] as const satisfies readonly (keyof LinksOps)[];

export const LINKS_COVERAGE = {
  "links.list": { webkit: { methods: ["links.list"] }, tauri: [], unsupported: { tauri: "links are the cua daemon's; the Tauri app has no share API" } },
  "links.create": { webkit: { methods: ["links.create"] }, tauri: [], unsupported: { tauri: "links are the cua daemon's; the Tauri app has no share API" } },
  "links.stop": { webkit: { methods: ["links.stop"] }, tauri: [], unsupported: { tauri: "links are the cua daemon's; the Tauri app has no share API" } },
  "links.approve": { webkit: { methods: ["links.approve"] }, tauri: [], unsupported: { tauri: "links are the cua daemon's; the Tauri app has no share API" } },
  "links.deny": { webkit: { methods: ["links.deny"] }, tauri: [], unsupported: { tauri: "links are the cua daemon's; the Tauri app has no share API" } },
  "shares.settings.get": {
    webkit: { methods: ["shares.settings.get"] },
    tauri: [],
    unsupported: { tauri: "shares are the cua daemon's; the Tauri app has no share API" },
  },
  "shares.settings.set": {
    webkit: { methods: ["shares.settings.set"] },
    tauri: [],
    unsupported: { tauri: "shares are the cua daemon's; the Tauri app has no share API" },
  },
  "shares.edge.remove": {
    webkit: { methods: ["shares.edge.remove"] },
    tauri: [],
    unsupported: { tauri: "shares are the cua daemon's; the Tauri app has no share API" },
  },
} as const satisfies Record<keyof LinksOps, OpCoverage>;

/** A host with no daemon-owned links. */
export function unsupportedLinksOps(mode: BridgeMode) {
  const no = (op: keyof LinksOps) => (): Promise<never> => Promise.reject(new UnsupportedOperationError(mode, op));
  return {
    "links.list": no("links.list"),
    "links.create": no("links.create"),
    "links.stop": no("links.stop"),
    "links.approve": no("links.approve"),
    "links.deny": no("links.deny"),
    "shares.settings.get": no("shares.settings.get"),
    "shares.settings.set": no("shares.settings.set"),
    "shares.edge.remove": no("shares.edge.remove"),
  };
}

const SAMPLE_SHARE_SETTINGS: ShareSettings = {
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
};

export const LINKS_ARGS = {
  "links.list": {},
  "links.create": { target: "space:dev:3000" },
  "links.stop": { id: "share-0123456789ab" },
  "links.approve": { id: "share-0123456789ab" },
  "links.deny": { id: "share-0123456789ab" },
  "shares.settings.get": {},
  "shares.settings.set": { settings: SAMPLE_SHARE_SETTINGS },
  "shares.edge.remove": {},
};
