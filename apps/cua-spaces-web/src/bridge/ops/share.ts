// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "Share": who a Space is shared with, and sharing it. Each operation is the
 * sharing command the native apps already run through the daemon (the
 * Tauri app's `space_shares`, `share_space` and `unshare_space`; the SwiftUI
 * app's `SpacesBackend.shares/share/unshare`). Sharing asks for presence in
 * the daemon (Touch ID or the login password) before anything reaches the
 * relay; nothing here asks or grants anything itself. The sheet's every
 * word and step is the app core's (`share.initial/reduce/view`).
 *
 * | Operation | Tauri command |
 * |---|---|
 * | `sharing.list` `{spaceId}` | `space_shares` |
 * | `sharing.share` `{spaceId, who, role}` | `share_space` (also changes a role) |
 * | `sharing.unshare` `{spaceId, who}` | `unshare_space` |
 *
 * Each answers the people the Space is shared with afterwards.
 *
 * **Native hosts** (the Electron shell answers the Swift host's methods): (`WebUIBridge+Pages.swift`): `sharing.list/share/unshare`
 * with the same args go to `SpacesBackend.shares/share/unshare(id:...)`, as
 * `ShareModel` calls them.
 */

import type { ShareEntry } from "../contracts/share";

export interface ShareOperations {
  "sharing.list": { args: { spaceId: string }; result: ShareEntry[] };
  "sharing.share": { args: { spaceId: string; who: string; role: string }; result: ShareEntry[] };
  "sharing.unshare": { args: { spaceId: string; who: string }; result: ShareEntry[] };
}

export type ShareOpName = keyof ShareOperations;
export type ShareHandlers = {
  [K in ShareOpName]: (args: ShareOperations[K]["args"]) => Promise<ShareOperations[K]["result"]>;
};

export const SHARE_OPERATIONS = ["sharing.list", "sharing.share", "sharing.unshare"] as const satisfies readonly ShareOpName[];

const row = <K extends ShareOpName>(op: K, tauri: string) => ({
  webkit: { methods: [op] as const },
  tauri: [tauri],
});

export const SHARE_COVERAGE = {
  "sharing.list": row("sharing.list", "space_shares"),
  "sharing.share": row("sharing.share", "share_space"),
  "sharing.unshare": row("sharing.unshare", "unshare_space"),
} as const;

/** `cua_spaces::share::SpaceShares`: the people are its `shares`. */
const people = (r: { shares?: ShareEntry[] } | null): ShareEntry[] => r?.shares ?? [];

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

/** The Tauri app's sharing commands (`src/native/share.ts`). */
export function tauriShareOps(invoke: Invoke): ShareHandlers {
  return {
    "sharing.list": async ({ spaceId }) => people(await invoke("space_shares", { spaceId })),
    "sharing.share": async ({ spaceId, who, role }) => people(await invoke("share_space", { spaceId, who, role })),
    "sharing.unshare": async ({ spaceId, who }) => people(await invoke("unshare_space", { spaceId, who })),
  };
}
