// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Space, SpaceAccess } from "@/bridge";
import type { SpaceState } from "@/lib/spaces";

/** One Space as the Spaces page draws it (a tile or a list row). */
export interface SpaceItem {
  space: Space;
  state: SpaceState;
  access: SpaceAccess;
  /** Yours (shared or not). */
  mine: boolean;
  /** Its size on its computer's disk, when known (yours only). */
  bytes: number | null;
  /** The computer it runs on, by name (filter box and fallbacks). */
  machine: string;
}
