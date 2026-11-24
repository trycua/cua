// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A machine that is online but whose owner stopped sharing it.
 *
 * The Machines page and New Space's "Run on" read one signal for online:
 * the relay sees the machine connected. A connected machine can still
 * refuse every call because its owner stopped sharing it (its relay record
 * says "not sharing", or its own policy does while the relay still lists
 * it as sharing). That is not offline, and both surfaces say so the same
 * way: the Machines page "Online, not sharing", Run on "(not sharing)".
 *
 * Hosts report it as an entry of the machine's `limits` (the one open list
 * `Spaces.hosts()` hands to every shell without a change of its shape),
 * `resource` {@link SHARING_STOPPED}: not a count, its `reason` is the line
 * to show. It is the app core's `HOST_SHARING_STOPPED`; the wizard reads it.
 * The page's own `Machine` has it as `notSharing`, and no limit.
 */

import type { MachineRow } from "./contracts/host";

type Limits = MachineRow["limits"];

/** `wizard::HOST_SHARING_STOPPED`. */
export const SHARING_STOPPED = "sharing";

/** The same words the SDK's create gives (`host_spaces::stopped_sharing`). */
export const stoppedSharingLine = (name: string) => `${name} stopped sharing: ask its owner to Resume sharing (or run \`cua host start\` there)`;

/** The relay's word (`cua spaces ls`' `status`) for a machine that is
 * connected but not shared. */
export const RELAY_NOT_SHARING = "not sharing";

/** What a connect to a machine that stopped sharing fails with, on this
 * Mac's probe of it: the SDK's own line, or the raw refusal
 * ("relay assertion refused: this machine stopped sharing"). */
export const STOPPED_SHARING_ERROR = /stopped sharing/i;

/** The line that says an online machine's owner stopped sharing it, if its
 * limits say so. */
export function notSharingOf(limits: Limits, online: boolean): string | undefined {
  if (!online) return undefined;
  const entry = limits.find((l) => l.resource === SHARING_STOPPED);
  if (!entry) return undefined;
  return entry.reason.trim().replace(/\.$/, "") || "Its owner stopped sharing it";
}

/** The machine's limits without the not-sharing entry. */
export function realLimits(limits: Limits): Limits {
  return limits.filter((l) => l.resource !== SHARING_STOPPED);
}

/** These limits, saying the machine's owner stopped sharing it (`line`). */
export function withNotSharing(limits: Limits, line: string): Limits {
  return [...realLimits(limits), { resource: SHARING_STOPPED, used: 0, limit: 0, reason: line }];
}
