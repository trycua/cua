// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { AccessLine } from "@/bridge";

/** Newest access lines one machine's page shows (`machines_account::RECENT_PER_MACHINE`). */
export const RECENT_PER_MACHINE = 5;

/** Who reached one machine, newest first: the lines for its id, or (a machine that only signed in) its sign-in's. */
export function accessFor(lines: readonly AccessLine[], machine: string, device: string | null): AccessLine[] {
  return lines.filter((l) => l.machine === machine || (device !== null && l.device === device && l.machine === null)).slice(0, RECENT_PER_MACHINE);
}
