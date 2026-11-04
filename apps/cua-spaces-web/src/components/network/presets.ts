// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The words and rules of the Network sheet.

import type { ClipboardMode, NetworkPreset, SpaceNetwork, SpaceTopology, VolumeScope } from "@/bridge/contracts/network";

export interface PresetInfo {
  id: NetworkPreset;
  title: string;
  /** Its name in a row or a menu. */
  short: string;
  description: string;
  /** Opens more than the default: says so when chosen. */
  warning?: string;
  /** How open it is: a child Space can't go above its parent's. */
  rank: number;
}

export const PRESETS: PresetInfo[] = [
  {
    id: "internet",
    title: "Internet only (default)",
    short: "Internet only",
    description: "Reaches the internet and its own child Spaces. Can't reach this computer, your local network or other Spaces.",
    rank: 1,
  },
  { id: "none", title: "No network", short: "No network", description: "No connections in or out. Cua can still show and control the Space.", rank: 0 },
  {
    id: "localhost",
    title: "Allow this computer's localhost",
    short: "This computer's localhost",
    description: "Lets the Space use services running on this computer, like a database or dev server.",
    warning: "Only for code you trust. Programs in this Space could read or change data in those services.",
    rank: 2,
  },
  {
    id: "lan",
    title: "Allow my local network",
    short: "Local network",
    description: "Lets the Space reach printers, routers, NAS and other computers on your network.",
    warning: "Only for code you trust. Programs in this Space could reach every computer on your network.",
    rank: 3,
  },
  { id: "custom", title: "Custom", short: "Custom", description: "Choose domains and address ranges.", rank: 1 },
];

export const presetInfo = (id: NetworkPreset) => PRESETS.find((p) => p.id === id)!;

/** "Internet only (default)", "No network", "Custom". */
export function networkSummary(n: SpaceNetwork): string {
  if (n.preset === "internet") return n.inheritedFrom ? "Internet only" : "Internet only (default)";
  return presetInfo(n.preset).short;
}

/** "Its own network", "This computer", a Space's name, or the proxy's address. */
export function exitLabel(via: string | undefined, members: SpaceTopology["members"] = []): string {
  if (!via || via === "internet") return "Its own network";
  if (via === "this-machine") return "This computer";
  if (via.startsWith("space:")) {
    const id = via.slice("space:".length);
    return members.find((m) => m.id === id || m.name === id)?.name.split(".")[0] ?? id.slice(id.lastIndexOf(":") + 1);
  }
  // A proxy, without any credentials in it.
  return via.replace(/\/\/[^@/]*@/, "//");
}

/** What an exit means, under the choice. */
export function exitDescription(via: string | undefined): string {
  if (!via || via === "internet") return "Internet traffic leaves through the Space's own network.";
  if (via === "this-machine") return "Internet traffic leaves through this computer's network, as if from this computer. The Space's rules still apply first.";
  if (via.startsWith("space:")) return "Internet traffic leaves through that Space, under that Space's rules too.";
  return "Internet traffic goes through this SOCKS5 proxy.";
}

/** A SOCKS5 proxy address the host takes. */
export const validProxy = (s: string) => /^socks5h?:\/\/([^@/\s]+@)?[a-z0-9.-]+(:\d{1,5})$/i.test(s.trim());

export const VOLUME_TEXT: Record<VolumeScope, string> = {
  "own-folder": "Its own folder only",
  granted: "Its folder and folders you allow",
  all: "The whole Volume",
  off: "Off",
};

export const CLIPBOARD_TEXT: Record<ClipboardMode, string> = {
  viewing: "Only while you're viewing",
  ask: "Ask each time",
  off: "Off",
};

/** What the platform blocks whatever the settings say, by address. */
const ALWAYS: [RegExp, string][] = [
  [/^169\.254\.169\.254$|^fd00:ec2::254$|^metadata(\.google\.internal)?$/i, "cloud metadata"],
  [/^169\.254\./, "link-local addresses"],
  [/(^|\.)cua\.(ai|internal)$/i, "Cua's own services"],
];

/** Why an entry can't be allowed ("cloud metadata"), or null. */
export function alwaysBlocked(entry: string): string | null {
  const host = entry.trim().replace(/\/\d+$/, "");
  return ALWAYS.find(([re]) => re.test(host))?.[1] ?? null;
}

/** A domain, `*.domain`, an IPv4 address or an IPv4 CIDR. */
export function validEntry(entry: string): boolean {
  const e = entry.trim();
  return /^(\*\.)?([a-z0-9-]+\.)+[a-z]{2,}$/i.test(e) || /^(\d{1,3}\.){3}\d{1,3}(\/\d{1,2})?$/.test(e);
}
