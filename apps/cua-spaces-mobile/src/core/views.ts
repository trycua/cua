// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** Typed views of the core's answers this app reads (camelCase, as the core serialises them). */

import { core } from "./index";

export type AgentTone = "you" | "not-connected" | "working" | "creating" | "failed" | "stopped" | "idle" | "asleep";

export interface ListMember {
  handle: string;
  harness: string;
  displayName: string;
  ownerId: string;
  status: string;
  activity: string | null;
  unread: number;
  lastMs: number;
  needsMs: number[];
  pending: boolean;
}
export interface ListSpace {
  spaceId: string;
  spaceName: string;
  members: ListMember[];
}
export interface AgentListRow {
  key: string;
  kind: "agent" | "attempt";
  label: string;
  harness: string;
  tone: AgentTone;
  line: string;
  job: string | null;
  lastMs: number;
  unread: number;
  places: { spaceId: string; spaceName: string; handle: string }[];
}
export interface AgentListView {
  rows: AgentListRow[];
  live: number;
  summary: string | null;
  title: string;
  seeAllLabel: string;
}

/** `agentList.view`: every agent as one list, in the core's order and words. */
export function agentList(spaces: ListSpace[]): AgentListView | null {
  return core.tryCall<AgentListView>("agentList.view", { input: { spaces, attempts: [] } });
}

export interface ListView {
  empty: boolean;
  reachingLine: string | null;
}

/** `spaces.listView`: "Loading Spaces…", "Reaching 2 machines…", or the empty state. */
export function listView(input: { loaded: boolean; incomplete: boolean; reaching: number; count: number }): ListView | null {
  return core.tryCall<ListView>("spaces.listView", { input });
}

export interface SignInFailure {
  kind: string;
  message: string;
  reason: string;
  detail?: string;
}

/** `signIn.failedFromError`: why a sign-in did not finish, in the core's words. */
export function signInFailure(message: string, variant?: string): SignInFailure | null {
  return core.tryCall<SignInFailure>("signIn.failedFromError", { message, ...(variant ? { variant } : {}) });
}

export interface SystemNote {
  id: string;
  title: string;
  body: string;
}
export interface NotificationInput {
  id: string;
  atMs: number;
  agent?: string | null;
  kind: "turn_ended" | "message" | "approval" | "error";
  title: string;
  body: string;
  read: boolean;
}

/** `notifications.plan`: which feed entries to post as system notifications. */
export function notificationsPlan(feed: NotificationInput[], seenMs: number): { post: SystemNote[]; seenMs: number } | null {
  return core.tryCall("notifications.plan", { feed, seenMs });
}

/** What a pool member says it can host (the core's `Capabilities`, camelCase). */
export interface Capabilities {
  v: number;
  canHostSpaces: boolean;
  images: string[];
  kinds: string[];
  desktop: boolean;
}

/** A computer or phone of the pool (`directory.view`): never a Space. */
export interface ComputerRow {
  id: string;
  name: string;
  /** `mac`, `windows`, `linux`, `cloud`, `iphone`, `ipad` or `phone`: the glyph. */
  kind: "mac" | "windows" | "linux" | "cloud" | "iphone" | "ipad" | "phone";
  /** `Online`, `Offline` or `Not sharing`. */
  state: string;
  online: boolean;
  line: string;
  note: string;
  hostsSpaces: boolean;
  spaces: number;
  isPhone: boolean;
  current: boolean;
  unconfirmed: boolean;
  capabilities: Capabilities;
}

/** A Space of the pool (`directory.view`). */
export interface DirectorySpace {
  id: string;
  machine: string;
  name: string;
  host: string;
  hostName: string;
  /** `Running`, `Stopped`, `Starting`, `Offline`, `Not sharing` or `Deleting`: the desktop's words. */
  state: string;
  online: boolean;
  os: string;
  kindLabel: string;
  role: "owner" | "editor" | "viewer";
  detail: string;
}

export interface DirectoryView {
  computers: ComputerRow[];
  spaces: DirectorySpace[];
  reachingLine: string | null;
  empty: { title: string; body: string } | null;
  errorLine: string | null;
}

export interface DirectoryInput {
  machines: unknown[] | null;
  devices: unknown[];
  error: string;
  reading: boolean;
  now: number;
}

/** `directory.view`: the account's Spaces and Computers from the relay directory, by the desktop's rules. */
export function directory(input: DirectoryInput): DirectoryView | null {
  return core.tryCall<DirectoryView>("directory.view", { input });
}

export interface PoolMember {
  id: string;
  online: boolean;
  hostsSpaces: boolean;
  capabilities: Capabilities;
}
export interface PlacementNeed {
  image?: string;
  kind?: string;
  desktop?: boolean;
}

/** `placement.offered`: the members a new Space can be placed on, by what each states it can host. */
export function placementOffered(members: PoolMember[], need: PlacementNeed = {}): string[] | null {
  return core.tryCall<string[]>("placement.offered", { members, need });
}
