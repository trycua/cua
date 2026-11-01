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
 * | `links.stop {id}` (`"all"` stops every link) | `Cua.stop_share`, `stop_all_shares` | the same | unsupported |
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
}

export interface LinkCreate {
  /** A port, `space:NAME:PORT`, or an absolute path on this machine. */
  target: string;
  access?: LinkAccess;
  ttlSeconds?: number;
  name?: string;
}

export interface LinksOps {
  "links.list": { args: Record<string, never>; result: { links: ShareLink[] } };
  "links.create": { args: LinkCreate; result: ShareLink };
  "links.stop": { args: { id: string }; result: { stopped: string[] } };
}

export const LINKS_OPERATIONS = ["links.list", "links.create", "links.stop"] as const satisfies readonly (keyof LinksOps)[];

export const LINKS_COVERAGE = {
  "links.list": { webkit: { methods: ["links.list"] }, tauri: [], unsupported: { tauri: "links are the cua daemon's; the Tauri app has no share API" } },
  "links.create": { webkit: { methods: ["links.create"] }, tauri: [], unsupported: { tauri: "links are the cua daemon's; the Tauri app has no share API" } },
  "links.stop": { webkit: { methods: ["links.stop"] }, tauri: [], unsupported: { tauri: "links are the cua daemon's; the Tauri app has no share API" } },
} as const satisfies Record<keyof LinksOps, OpCoverage>;

/** A host with no daemon-owned links. */
export function unsupportedLinksOps(mode: BridgeMode) {
  const no = (op: keyof LinksOps) => (): Promise<never> => Promise.reject(new UnsupportedOperationError(mode, op));
  return { "links.list": no("links.list"), "links.create": no("links.create"), "links.stop": no("links.stop") };
}

export const LINKS_ARGS = {
  "links.list": {},
  "links.create": { target: "space:dev:3000" },
  "links.stop": { id: "share-0123456789ab" },
};
