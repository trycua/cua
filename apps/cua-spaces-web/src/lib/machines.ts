// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Machine, SpaceOs } from "@/bridge";
import { machineSpacesLabel } from "@/bridge/derive";
import { OS_NAME } from "@/components/os-icon";
import { t } from "@/i18n";
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
  return os ? OS_NAME[os] : t("computers.row.unknown_os");
}

const connectionWord = (c: "relay" | "direct"): string => (c === "relay" ? t("computers.row.connection_relay") : t("computers.row.connection_direct"));

/** A row that is only a signed-in machine (`machines_view::DEVICE_LINE`). */
const deviceLine = (): string => t("computers.row.device_line");

/** How other machines reach it, in words. */
export function connectionLabel(m: Pick<Machine, "connection" | "current"> & Pick<Partial<Machine>, "device">): string {
  if (m.connection) return connectionWord(m.connection);
  if (m.device) return deviceLine();
  return m.current ? t("computers.row.not_set_up") : t("computers.row.unknown");
}

/**
 * The second line of this computer's card on a task: its model ("MacBook Pro") or else its OS, never the
 * card's own title again ("This Mac" / "This Mac"). Null when nothing more is known.
 */
export function thisComputerSubtitle(machines: readonly Pick<Machine, "os" | "model" | "current">[] | undefined, title: string): string | null {
  const m = machines?.find((x) => x.current);
  if (!m) return null;
  const line = m.model?.trim() || (machineOs(m) ? osName(m) : "");
  return line && line !== title ? line : null;
}

/** The list row's second line: OS, then how it is reached. */
export function machineSubtitle(m: Pick<Machine, "os" | "connection" | "current" | "model"> & Pick<Partial<Machine>, "device">): string {
  return [m.model ?? osName(m), connectionLabel(m)].join(", ");
}

/** The core's word for a machine its owner stopped sharing
 * (`machines::NOT_SHARING`): connected to the relay, but it refuses every
 * call. */
const notSharing = (): string => t("computers.row.not_sharing");

/** Usable from here: online and shared (a machine whose owner stopped
 * sharing it is connected, but nothing here can reach it). */
export const reachable = (m: Pick<Machine, "online" | "notSharing">): boolean => m.online && !m.notSharing;

/** The detail's line: the row's word, and when an offline machine was
 * last seen. */
export function presenceLabel(m: Pick<Machine, "online" | "lastSeen" | "notSharing" | "status">, now: number): string {
  if (m.online || !m.lastSeen) return presenceWord(m);
  return t("computers.row.offline_last_seen", { when: relativeTime(m.lastSeen * 1000, now) });
}

/** The list row's one word: the core's (`machines::status_word`, the same
 * the SwiftUI Machines list shows): "Online", "Not sharing" or "Offline". */
export function presenceWord(m: Pick<Machine, "online" | "notSharing" | "status">): string {
  if (m.status) return m.status;
  if (!m.online) return t("computers.row.offline");
  return m.notSharing ? notSharing() : t("computers.row.online");
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

/**
 * The computers a chat could carry on on ("Continue on"): your other
 * computers that are online and shared, each once. Signed-in devices that
 * aren't computers (a web browser, a phone) can't run a chat, nor can an
 * offline one; `offline` counts the computers left out for being offline.
 */
export function chatComputers(machines: readonly Machine[]): { computers: Machine[]; offline: number } {
  const seen = new Set<string>();
  const others: Machine[] = [];
  for (const m of machines) {
    if (m.current || m.device || seen.has(m.id)) continue;
    seen.add(m.id);
    others.push(m);
  }
  const computers = others.filter(reachable);
  return { computers, offline: others.length - computers.length };
}

/** The id `/spaces/computer/$computerId` takes for this computer, whatever the relay calls it. */
export const THIS_COMPUTER = "this-mac";

/** The computer a page id names: `this-mac` for this one, else its own id. */
export function computerFor<T extends { id: string; current: boolean }>(machines: readonly T[], id: string): T | undefined {
  if (id === THIS_COMPUTER) return machines.find((m) => m.current);
  return machines.find((m) => m.id === id);
}

/** The page path for a computer in your list. */
export function computerParam(m: Pick<Machine, "id" | "current">): string {
  return m.current ? THIS_COMPUTER : m.id;
}
