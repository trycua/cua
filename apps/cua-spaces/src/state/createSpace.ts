// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { core } from "../core";
import type { CreatePlan } from "../components/desktop/NewSpaceWizard";
import type { FleetSync } from "./cloud";

/**
 * Turns a New Space plan into the SDK call that creates it: one
 * `create_space` with where (`on`), what (`kind`, from the image list) and
 * the engine the person chose (`auto` unless they picked one). Resolves with
 * the Space id; until then the Space shows as `pendingId`'s row.
 */
export function createFromPlan(
  sync: Pick<FleetSync, "createSpace">,
  plan: CreatePlan,
  pendingId?: string,
): Promise<string> {
  const args = core<{
    image: string;
    /** `local`, `cloud`, or a connected cloud's word (`aws`). */
    on: string;
    kind: "container" | "vm";
    runtime: CreatePlan["runtime"];
    name?: string;
    cpus?: number;
    memoryMb?: number;
    diskGb?: number;
    spacesd: boolean;
    gpu?: string;
  }>("wizard.createArgs", { plan });
  return sync.createSpace(args, pendingId);
}
