// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Secure-by-default network, Volume and clipboard settings per Space, as
 * the Space page reads them: from the host (`bridge/network.ts`, over the
 * `network.*` and `spaces.setAccess` operations) or from the demo's memory
 * (`?demo=network`).
 */

/**
 * What a Space may reach. `internet` is the default. `localhost` and `lan` are each
 * Internet only plus one extra; `localhost+lan` has both.
 */
export type NetworkPreset = "internet" | "none" | "localhost" | "lan" | "localhost+lan" | "custom";

export interface SpaceNetwork {
  preset: NetworkPreset;
  /** Custom: domains, wildcards (`*.github.com`) and CIDRs the Space may reach. */
  allow: string[];
  /** Custom: what it may never reach, on top of the platform's own blocks. */
  deny: string[];
  /** Custom's starting point: everything not listed is allowed (`internet`) or blocked (`none`). */
  base?: "internet" | "none";
  /** Localhost: only these ports on this computer. Empty: every port. */
  localhostPorts?: number[];
  /** The parent Space whose settings this one takes; it can only be stricter. */
  inheritedFrom?: string;
  /** The daemon enforces it (false: the runtime can't, and the UI says so). */
  enforced: boolean;
  /** Where its internet traffic leaves: `internet` (its own network, the
   * default), `this-machine`, `space:<id>` (a Space of its tree) or
   * `socks5://host:port`. Its rules apply first; if the exit goes away its
   * internet stops. A child keeps its parent's. */
  via?: string;
}

/** Where a Space is reachable and how its traffic leaves, as the host
 * reports it (`cua network show --json`'s `topology`). */
export interface SpaceTopology {
  /** `<space>.<tree>`: its tree reaches it by this name. */
  name?: string;
  /** Its exit and how it's doing. */
  exit?: { label: string; state: "active" | "starting" | "waiting_for_peer" | "unreachable" | string; note?: string };
  /** Where this computer's open ports answer in the Space (`localhost:3000`, or `host:3000`). */
  hostReach: string[];
  /** Ports answering at `host:<port>` only, and why. */
  hostOnly: { port: number; why: string }[];
  /** The other Spaces of its tree (exit choices). */
  members: { id: string; name: string; relation: string }[];
  /** Members of its tree that run on your other computers: their name and the machine's. */
  remote?: { name: string; machine: string }[];
  /** How traffic reaches your other computers. */
  paths: { name: string; kind: "lan" | "direct" | "relay" | string; why: string }[];
}

/** What a Space may see of the Cua Volume. */
export type VolumeScope = "own-folder" | "granted" | "all" | "off";

/** When the Space's clipboard reaches yours. */
export type ClipboardMode = "viewing" | "ask" | "off";

export interface SpaceSecurity {
  network: SpaceNetwork;
  volume: VolumeScope;
  clipboard: ClipboardMode;
  /** The parent Space's id and name, for a child Space. */
  parent?: { id: string; name: string };
  /** Its child Spaces' names. */
  children: string[];
  /** An update changed its network to the new default: say so once. */
  noticePending?: boolean;
  /** One plain line ("Internet only"), as the host says it. */
  summary?: string;
  /** Where its rules are enforced, or why it has no network. */
  point?: string;
  /** Why its network is not what was asked. */
  note?: string;
  /** Its name, exit, host ports and paths; absent from an older host. */
  topology?: SpaceTopology;
}

/** A connection the Space tried that was blocked. */
export interface BlockedConnection {
  address: string;
  /** "This computer's localhost", "Cloud metadata", "Local network". */
  what: string;
  /** Epoch ms. */
  at: number;
  /** Blocked by the platform whatever the settings say. */
  always: boolean;
}

/** An agent in a Space asks for a folder of the Volume. */
export interface VolumeRequest {
  id: string;
  spaceId: string;
  path: string;
  who: string;
}

/** The Space copied text while you watch it. */
export interface ClipboardOffer {
  spaceId: string;
  chars: number;
}

/** The demo host's store (`DataAdapter.network`). */
export interface NetworkPreview {
  get(spaceId: string): SpaceSecurity | null;
  /** `widen`: a child Space may reach more than its parent (the user chose
   * "Allow more for this Space only"). */
  set(spaceId: string, network: SpaceNetwork, opts?: { widen?: boolean }): void;
  blocked(spaceId: string): BlockedConnection[];
  /** The newest blocked connection to show on the Space page, until dismissed. */
  notice(spaceId: string): BlockedConnection | null;
  dismissNotice(spaceId: string): void;
  volumeRequest(spaceId: string): VolumeRequest | null;
  answerVolume(id: string): void;
  clipboardOffer(spaceId: string): ClipboardOffer | null;
  answerClipboard(spaceId: string): void;
  /** Sets a Space's clipboard or Volume setting (a host that keeps them). */
  setAccess?(spaceId: string, access: { clipboard?: ClipboardMode; volume?: VolumeScope }): void;
  /** The one-time notice about the new network default was shown. */
  ackNotice?(spaceId: string): void;
  /** The last change to a Space's network the host refused, in its words. */
  error?(spaceId: string): string | null;
  /** New Space's choice. */
  wizardPreset: NetworkPreset;
  setWizardPreset(preset: NetworkPreset): void;
  subscribe(listener: () => void): () => void;
  /** Bumps on every change (for `useSyncExternalStore`). */
  version(): number;
}
