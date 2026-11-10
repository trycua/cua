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
}

/** What a preset adds to Internet only. The two extras are independent of each other. */
export interface Extras {
  /** Services running on this computer, on listed ports. */
  localhost: boolean;
  /** Printers, routers, NAS and other devices on the local network. */
  lan: boolean;
}

export const extrasOf = (p: NetworkPreset): Extras => ({ localhost: p === "localhost" || p === "localhost+lan", lan: p === "lan" || p === "localhost+lan" });

/** The Internet-only preset with these extras: `internet`, `localhost`, `lan` or `localhost+lan`. */
export const presetWith = ({ localhost, lan }: Extras): NetworkPreset =>
  localhost && lan ? "localhost+lan" : localhost ? "localhost" : lan ? "lan" : "internet";

/** Internet only with the extras ("Also allow"), or something else. */
export const isInternetBase = (p: NetworkPreset) => p === "internet" || p === "localhost" || p === "lan" || p === "localhost+lan";

/**
 * Whether `p` opens more than `ceiling`, a parent's preset: a child Space can't. Having
 * any network is more than none; each extra is more only where the ceiling lacks it, so
 * neither extra is above the other.
 */
export function exceeds(p: NetworkPreset, ceiling: NetworkPreset): boolean {
  const reaches = (x: NetworkPreset) => (x === "none" ? 0 : 1);
  if (reaches(p) > reaches(ceiling)) return true;
  const a = extrasOf(p);
  const c = extrasOf(ceiling);
  return (a.localhost && !c.localhost) || (a.lan && !c.lan);
}

/** The two "Also allow" choices under Internet only. */
export const EXTRAS: { id: keyof Extras; title: string; description: string; warning: string }[] = [
  {
    id: "localhost",
    title: "Services on this computer",
    description: "Lets the Space use services running on this computer, like a database or dev server.",
    warning: "Only for code you trust. Programs in this Space could read or change data in those services.",
  },
  {
    id: "lan",
    title: "Other devices on my network",
    description: "Lets the Space reach printers, routers, NAS and other computers on your network.",
    warning: "Only for code you trust. Programs in this Space could reach every computer on your network.",
  },
];

export const PRESETS: PresetInfo[] = [
  {
    id: "internet",
    title: "Internet only",
    short: "Internet only",
    description: "Reaches the internet and its own child Spaces. Can't reach this computer, your local network or other Spaces.",
  },
  { id: "none", title: "No network", short: "No network", description: "No connections in or out. Cua can still show and control the Space." },
  {
    id: "localhost",
    title: "Allow this computer's localhost",
    short: "This computer's localhost",
    description: "Lets the Space use services running on this computer, like a database or dev server.",
    warning: "Only for code you trust. Programs in this Space could read or change data in those services.",
  },
  {
    id: "lan",
    title: "Allow my local network",
    short: "Local network",
    description: "Lets the Space reach printers, routers, NAS and other computers on your network.",
    warning: "Only for code you trust. Programs in this Space could reach every computer on your network.",
  },
  {
    id: "localhost+lan",
    title: "Allow this computer's localhost and my local network",
    short: "Localhost and local network",
    description: "Lets the Space use services on this computer and reach the other devices on your network.",
    warning: "Only for code you trust. Programs in this Space could use services on this computer and reach every computer on your network.",
  },
  { id: "custom", title: "Custom", short: "Custom", description: "Choose domains and address ranges." },
];

export const presetInfo = (id: NetworkPreset) => PRESETS.find((p) => p.id === id)!;

/** "Internet only", "Internet and this computer's services", "No network", "Custom". */
export function networkSummary(n: SpaceNetwork): string {
  const e = extrasOf(n.preset);
  if (e.localhost && e.lan) return "Internet, this computer's services and your local network";
  if (e.localhost) return "Internet and this computer's services";
  if (e.lan) return "Internet and your local network";
  if (n.preset === "internet") return "Internet only";
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
