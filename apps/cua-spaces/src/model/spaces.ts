// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The cua SDK's Spaces registry, mapped onto the portal's Space model.
 *
 * Every location (Cua Cloud, this Mac, Spaces added by address) arrives
 * through one `list_spaces` roster. The SDK owns lifecycle, the
 * runtime/image pairing rules and the spacesd handshake; this module only
 * turns its rows into tiles.
 */

import { core } from "../core";
import type { Space, SpaceOs, SpaceProvider, ThumbnailScene } from "./types";

export const SPACES_CONFIG = {
  /** Thumbnail refresh cadence for visible running tiles (~0.3 fps). */
  thumbnailIntervalMs: 3_000,
  /** Registry refresh cadence while the portal is open. */
  refreshMs: 10_000,
  /** Long edge of tile thumbnails, in pixels. */
  thumbnailMaxDimension: 480,
} as const;

/** One row of the shell's `list_spaces` (see CONTRACT.md `SpaceRow`). */
export interface SpaceRow {
  id: string;
  name: string;
  /** The SDK's location word: `cloud`, `local`, `direct` or `relay`. */
  provider: SpaceProvider;
  spacesdVersion: string;
  features: string[];
  addedAt?: string;
  os?: SpaceOs;
  /** OS product or distribution spacesd reported ("Ubuntu"). */
  osName?: string;
  /** The full OS string ("Ubuntu 24.04.3 LTS"), when reported. */
  osPrettyName?: string;
  /** The image it runs and its digest, when known. */
  image?: string;
  imageDigest?: string;
  /** `container` or `vm`, and the guest's CPU architecture, when known. */
  kind?: "container" | "vm";
  arch?: string;
  reachable: boolean;
  error?: string;
  /** For a Space one of your machines provides: that machine's relay id and name. */
  host?: string;
  hostName?: string;
  /** How it turns off and on (`suspend`, `stop`); absent when it cannot. */
  power?: string;
  /** `running`, `suspended` or `stopped` as cua last left it, when known. */
  powerState?: string;
  /** For a Space in your cloud: the provider word (`aws`, `gcp`, `modal`),
   * its account and region in words ("AWS · us-west-2"), and where Delete
   * Permanently can delete it (`here`, `host:<machine>`, `elsewhere`). */
  cloud?: string;
  cloudPlace?: string;
  cloudDelete?: string;
}

/** spacesd feature names the UI keys on. */
export const FEATURE = {
  desktopStream: "desktop_stream",
  windowStream: "window_stream",
  audioDesktop: "audio.desktop",
  hotspot: "hotspot",
} as const;

export function hasFeature(space: Pick<Space, "sdk">, feature: string): boolean {
  return Boolean(space.sdk?.features.includes(feature));
}

/** The provider a Space id names (`cloud:<name>`, `space://<loc>/…`). */
export function providerOfId(id: string): SpaceProvider | undefined {
  return core<SpaceProvider | null>("spaces.providerOfId", { id }) ?? undefined;
}

/** The app's location for a row's provider word. */
export function normalizeProvider(word: string): SpaceProvider {
  return core("spaces.normalizeProvider", { word });
}

/** The cloud namespace of a legacy `space://cloud/<ns>/<name>` id. */
export function cloudNamespaceOf(id: string): string | undefined {
  return core<string | null>("spaces.cloudNamespaceOf", { id }) ?? undefined;
}

export function sceneForOs(os: SpaceOs): ThumbnailScene {
  return core("spaces.sceneForOs", { os });
}

/** Human-friendly name ("brave-otter" -> "Brave Otter"; `host:port` stays). */
export function displayName(name: string): string {
  return core("spaces.displayName", { name });
}

/** The name to show (a container's 12-hex hostname defers to the id). */
export function nameOf(row: Pick<SpaceRow, "id" | "name">): string {
  return core("spaces.nameOf", { id: row.id, name: row.name });
}

/** Map one registry row onto the Space model (the app core decides). */
export function rowToSpace(row: SpaceRow, now: number): Space {
  return core("spaces.rowToSpace", { row, now });
}
