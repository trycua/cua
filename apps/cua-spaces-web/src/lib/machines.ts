// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Machine, SpaceOs } from "@/bridge";
import { machineSpacesLabel } from "@/bridge/derive";
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

/** A row that is only a signed-in device (`machines_view::DEVICE_LINE`). */
export const DEVICE_LINE = "Signed-in device, not shared";

/** How other devices reach it, in words. */
export function connectionLabel(m: Pick<Machine, "connection" | "current"> & Pick<Partial<Machine>, "device">): string {
  if (m.connection) return CONNECTION_LABEL[m.connection];
  if (m.device) return DEVICE_LINE;
  return m.current ? "Not set up for access" : "Unknown";
}

/** The list row's second line: OS, then how it is reached. */
export function machineSubtitle(m: Pick<Machine, "os" | "connection" | "current" | "model"> & Pick<Partial<Machine>, "device">): string {
  return [m.model ?? osName(m), connectionLabel(m)].join(" · ");
}

/** The core's word for a machine its owner stopped sharing
 * (`machines::NOT_SHARING`): connected to the relay, but it refuses every
 * call. */
export const NOT_SHARING = "Not sharing";

/** Usable from here: online and shared (a machine whose owner stopped
 * sharing it is connected, but nothing here can reach it). */
export const reachable = (m: Pick<Machine, "online" | "notSharing">): boolean => m.online && !m.notSharing;

/** The detail's line: the row's word, and when an offline machine was
 * last seen. */
export function presenceLabel(m: Pick<Machine, "online" | "lastSeen" | "notSharing" | "status">, now: number): string {
  if (m.online || !m.lastSeen) return presenceWord(m);
  return `Offline, last seen ${relativeTime(m.lastSeen * 1000, now)}`;
}

/** The list row's one word: the core's (`machines::status_word`, the same
 * the SwiftUI Machines list shows): "Online", "Not sharing" or "Offline". */
export function presenceWord(m: Pick<Machine, "online" | "notSharing" | "status">): string {
  if (m.status) return m.status;
  if (!m.online) return "Offline";
  return m.notSharing ? NOT_SHARING : "Online";
}

/** "1 Space", "3 Spaces", "No Spaces". */
export function spaceCount(n: number): string {
  return machineSpacesLabel(false, n);
}

/** The row's Spaces count: the core's (`machines::spaces_label`, the same
 * the SwiftUI Machines list shows), else `n` counted here. Empty for a
 * device of its own (it runs no Spaces). `desktop`: the machine shares its
 * own desktop ("Desktop shared", "Desktop and 2 Spaces"). */
export function spacesLabel(m: Pick<Machine, "spacesLabel">, n: number, desktop = false): string {
  return m.spacesLabel ?? machineSpacesLabel(desktop, n);
}

/** The selected machine: the chosen one if it is still listed, else the first. */
export function selectedMachine<T extends Pick<Machine, "id">>(machines: readonly T[], id: string | null): T | undefined {
  return machines.find((m) => m.id === id) ?? machines[0];
}

/** The Machines header: "1 machine, 1 online", "3 machines, 2 online". */
export function machinesSummary(total: number, online: number): string {
  return `${plural(total, "machine")}, ${online} online`;
}
