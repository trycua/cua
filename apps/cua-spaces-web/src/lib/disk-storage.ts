// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { DiskKind } from "@/bridge/contracts/settings";

/** The folders a person can move, in the order Settings lists them. */
export const MOVABLE_KINDS = ["runtimes", "images", "cache", "logs", "trajectories"] as const;
export type MovableKind = (typeof MOVABLE_KINDS)[number];

/** `11.3 GB`, `512 MB`, `0 KB`: decimal units, as Finder shows them. */
export function diskBytes(bytes: number): string {
  if (bytes >= 1e12) return `${(bytes / 1e12).toFixed(1)} TB`;
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`;
  return `${Math.round(bytes / 1e3)} KB`;
}

/** `/Users/maya/.cua/cache` as `~/.cua/cache` under `home`. */
export function tildePath(path: string, home: string | null): string {
  if (!home) return path;
  const h = home.replace(/\/+$/, "");
  if (path === h) return "~";
  return path.startsWith(`${h}/`) ? `~${path.slice(h.length)}` : path;
}

/** The home folder of the user, taken from the cua home (`~/.cua`, `~/.cua-local`, `~/.cua/envs/staging`). */
export function userHomeOf(cuaHome: string): string | null {
  const m = cuaHome.match(/^(.*?)\/\.cua(?:-local)?(?:\/|$)/);
  return m?.[1] ? m[1] : null;
}

/** The movable kinds present in the host's answer, in display order. */
export function movableKinds(kinds: DiskKind[]): DiskKind[] {
  return MOVABLE_KINDS.map((k) => kinds.find((x) => x.kind === k && x.movable)).filter((k): k is DiskKind => Boolean(k));
}
