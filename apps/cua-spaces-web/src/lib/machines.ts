// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Machine, SpaceOs } from "@/bridge";
import { OS_NAME } from "@/components/os-icon";
import { plural } from "@/lib/plural";
import { relativeTime } from "@/lib/utils";

/** The machine's OS as one of the three the app draws, or null when the host did not say. */
export function machineOs(m: Pick<Machine, "os">): SpaceOs | null {
  const os = m.os.toLowerCase();
  if (os === "macos" || os === "darwin" || os === "mac") return "macos";
  if (os === "windows" || os === "win32") return "windows";
  if (os === "linux") return "linux";
  return null;
}

/** "macOS", or "Unknown OS". */
export function osName(m: Pick<Machine, "os">): string {
  const os = machineOs(m);
  return os ? OS_NAME[os] : "Unknown OS";
}

export const CONNECTION_LABEL: Record<"relay" | "direct", string> = {
  relay: "Cua relay",
  direct: "Direct",
};

/** How other devices reach it, in words. */
export function connectionLabel(m: Pick<Machine, "connection" | "current"> & Pick<Partial<Machine>, "device">): string {
  if (m.connection) return CONNECTION_LABEL[m.connection];
  return m.current || m.device ? "Not set up for access" : "Unknown";
}

/** The list row's second line: OS, then how it is reached. */
export function machineSubtitle(m: Pick<Machine, "os" | "connection" | "current" | "model"> & Pick<Partial<Machine>, "device">): string {
  return [m.model ?? osName(m), connectionLabel(m)].join(" · ");
}

/** The core's word for a machine its owner stopped sharing: it cannot be
 * reached from here (`sidebar::detail_live`'s preview line). */
export const NOT_REACHABLE = "Not reachable";

/** Usable from here: online and shared (a machine whose owner stopped
 * sharing it is connected, but nothing here can reach it). */
export const reachable = (m: Pick<Machine, "online" | "notSharing">): boolean => m.online && !m.notSharing;

/** "Online", "Not reachable" (its owner stopped sharing it; `reach` is the
 * core's word for its Space, as the SwiftUI detail shows it), or "Offline"
 * with when it was last seen. */
export function presenceLabel(m: Pick<Machine, "online" | "lastSeen" | "notSharing">, now: number, reach?: string | null): string {
  if (m.online) return m.notSharing ? (reach ?? NOT_REACHABLE) : "Online";
  return m.lastSeen ? `Offline, last seen ${relativeTime(m.lastSeen * 1000, now)}` : "Offline";
}

/** The list row's one word: "Online", "Not reachable" or "Offline". */
export function presenceWord(m: Pick<Machine, "online" | "notSharing">, reach?: string | null): string {
  if (!m.online) return "Offline";
  return m.notSharing ? (reach ?? NOT_REACHABLE) : "Online";
}

/** "1 Space", "3 Spaces", "No Spaces". */
export function spaceCount(n: number): string {
  if (n === 0) return "No Spaces";
  return `${n} ${n === 1 ? "Space" : "Spaces"}`;
}

/** The selected machine: the chosen one if it is still listed, else the first. */
export function selectedMachine<T extends Pick<Machine, "id">>(machines: readonly T[], id: string | null): T | undefined {
  return machines.find((m) => m.id === id) ?? machines[0];
}

/** The Machines header: "1 machine, 1 online", "3 machines, 2 online". */
export function machinesSummary(total: number, online: number): string {
  return `${plural(total, "machine")}, ${online} online`;
}
