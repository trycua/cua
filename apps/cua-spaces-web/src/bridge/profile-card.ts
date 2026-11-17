// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The card a person or an agent opens to in a Space: the app core's `profileCard.*` (`profile_card.rs`). */

import type { CoreClient } from "./core";
import type { ConnectorsInput } from "./connectors";

type Json = Record<string, unknown>;

export interface CardDetail {
  label: string;
  value: string;
  copy: boolean;
}

export interface PersonCardInput {
  name: string;
  who: string | null;
  me: boolean;
  access: string | null;
  team: string | null;
  active: boolean;
  lastSeenMs: number | null;
  nowMs: number;
  canManage: boolean;
}

export interface PersonCardView {
  name: string;
  youLabel: string | null;
  email: string | null;
  access: string | null;
  team: string | null;
  presence: string | null;
  active: boolean;
  changeAccessLabel: string | null;
  accessChoices: { value: string; label: string }[];
  accessValue: string | null;
  removeLabel: string | null;
}

export interface AgentCardInput {
  displayName: string;
  handle: string;
  harness: string;
  harnessReported: string | null;
  ownerName: string;
  ownerId: string;
  ownerIsMe: boolean;
  device: string | null;
  origin: string;
  status: string;
  activity: string | null;
  joinedHow: string;
  joinedAtMs: number | null;
  nowMs: number;
  rights: { label: string; holders: string }[];
  /** Every right is at its default: one "Default permissions" line instead of three rows. */
  rightsDefault: boolean;
  /** You may change its rights (the Space's owner and admins, and its owner). */
  canEditRights: boolean;
  canRemove: boolean;
  connectors: ConnectorsInput;
}

export interface AgentCardView {
  name: string;
  badge: string;
  line: string;
  status: "working" | "idle" | "not-connected" | "pending";
  statusText: string;
  connectLabel: string | null;
  app: string | null;
  settingsLabel: string | null;
  removeLabel: string | null;
  detailsLabel: string;
  details: CardDetail[];
  /** "Owner": who runs it is a person, shown by name (never an id). */
  ownerLabel: string;
  ownerId: string;
  ownerName: string;
  /** "Default permissions", while every right is at its default. */
  defaultPermissions: string | null;
  /** "Edit", beside it, for those who may change the rights. */
  editRightsLabel: string | null;
}

export const personCard = (core: CoreClient, input: PersonCardInput): PersonCardView | null =>
  core.tryCall<PersonCardView>("profileCard.person", { input: input as unknown as Json }) ?? null;

export const agentCard = (core: CoreClient, input: AgentCardInput): AgentCardView | null =>
  core.tryCall<AgentCardView>("profileCard.agent", { input: input as unknown as Json }) ?? null;
