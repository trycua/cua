// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { core } from "../core";
import type { Space } from "./types";

/** A Space on the Cua Spaces pool (listed first). */
export function isSpacesPool(space: Space): boolean {
  return core("spaces.isSpacesPool", { space });
}

/** Pool Spaces first, then most recently used, then by name. */
export function sortByMru(spaces: readonly Space[]): Space[] {
  return core("spaces.sortByMru", { spaces });
}

/** Mark a Space as used at `now` (unknown ids are a no-op). */
export function touchSpace(spaces: readonly Space[], id: string, now: number): Space[] {
  return core("spaces.touchSpace", { spaces, id, now });
}

/** Number of Spaces that count as "active". */
export function countActive(spaces: readonly Space[]): number {
  return core("spaces.countActive", { spaces });
}

/** The three ambient status dots. */
export interface AmbientDots {
  running: boolean;
  approval: boolean;
  suspended: boolean;
  /** True when any agent is actively working (drives the subtle pulse). */
  agentActive: boolean;
}

export function ambientDots(spaces: readonly Space[]): AmbientDots {
  return core("spaces.ambientDots", { spaces });
}
