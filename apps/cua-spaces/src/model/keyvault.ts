// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { core } from "../core";
import type {
  KeyvaultOverview,
  KvAuditEntry,
  KvCaller,
  KvDelivery,
  KvGrant,
  KvItem,
  KvPending,
  KvRule,
} from "../native/keyvault";

/**
 * Pure shaping for the Keyvault page: items grouped per site and per
 * account, each with its consent state, and the recent decisions from the
 * append-only audit log. Everything is derived from what the broker
 * returned; nothing here decides access. The shaping is the app core's
 * (`cua-spaces-app-core::keyvault`), shared with the SwiftUI app.
 */

export type ConsentKind = "pending" | "delivered" | "granted" | "rule" | "asks";

export interface ConsentChip {
  kind: ConsentKind;
  text: string;
}

export interface ItemRow {
  item: KvItem;
  /** "ada@example.com", "Whole app session", or the profile. */
  account: string;
  /** Strongest state first; `asks` when nothing else applies. */
  consent: ConsentChip[];
  /** The unattended switch can be changed (not an identity provider). */
  toggleEnabled: boolean;
  /** The switch's tooltip. */
  toggleHelp: string;
}

export interface SiteGroup {
  key: string;
  /** "github.com" or the app ("Slack"). */
  title: string;
  /** "Chrome", "Firefox", "Slack". */
  app: string;
  rows: ItemRow[];
  /** All rows allowed unattended, none, or some. */
  unattended: "on" | "off" | "mixed";
  /** Identity providers never run unattended. */
  locked: boolean;
}

export function liveGrant(g: KvGrant, now: number): boolean {
  return core("keyvault.liveGrant", { grant: g, now });
}

export function liveRule(r: KvRule, now: number): boolean {
  return core("keyvault.liveRule", { rule: r, now });
}

export function liveDelivery(d: KvDelivery, now: number): boolean {
  return core("keyvault.liveDelivery", { delivery: d, now });
}

export function duration(ms: number): string {
  return core("keyvault.duration", { ms: Math.round(ms) });
}

/** A short name for a verified caller display. */
export function shortCaller(display: string): string {
  return core("keyvault.shortCaller", { display });
}

export function signingBadge(c: KvCaller): { text: string; tone: "ok" | "warn" | "danger" } {
  return core("keyvault.signingBadge", { caller: c });
}

/** Items grouped per site (browsers) or per app (whole-app sessions). */
export function groupItems(o: KeyvaultOverview, now: number, query = ""): SiteGroup[] {
  const byId = new Map(o.items.map((i) => [i.id, i]));
  return core<SiteGroup[]>("keyvault.groupItems", { overview: o, now, query }).map((g) => ({
    ...g,
    rows: g.rows.map((r) => ({ ...r, item: byId.get(r.item.id) ?? r.item })),
  }));
}

/** Audit kinds the "Recent decisions" list shows. */
export const DECISION_KINDS: Record<string, string> = Object.freeze({
  "consent.request": "Asked",
  "consent.allow": "Approved",
  "consent.deny": "Denied",
  "teleport.deliver": "Delivered",
  "teleport.deny": "Refused",
  "grant.revoke": "Revoked",
  "target.wipe": "Wiped",
  "rule.add": "Rule added",
  "rule.remove": "Rule removed",
  "item.policy": "Policy changed",
  "vault.disable": "Keyvault turned off",
  "vault.enable": "Keyvault turned on",
  "caller.reject": "Caller refused",
}) as Record<string, string>;

export interface Decision {
  entry: KvAuditEntry;
  verb: string;
  /** `allow`, `deny`, `ok` or `error`, for the tone. */
  tone: "ok" | "deny" | "info";
  what: string;
}

/** Newest first. The log is append-only; this only filters and labels. */
export function recentDecisions(o: KeyvaultOverview, limit = 12): Decision[] {
  return core("keyvault.recentDecisions", { overview: o, limit });
}

/** What a pending request would move, as one line. */
export function pendingSummary(p: KvPending): string {
  return core("keyvault.pendingSummary", { pending: p });
}

/* ---- Approval sheet: nothing is selected by default ---------------------- */

export interface ApprovalState {
  requestId: string;
  selected: string[];
}

export type ApprovalAction = { type: "toggle"; key: string } | { type: "select-all" } | { type: "clear" };

export interface ApprovalView {
  requestId: string;
  title: string;
  caller: string;
  badge: { text: string; tone: "ok" | "warn" | "danger" };
  targets: string;
  wants: string;
  summary: string;
  claims: string[];
  rows: { key: string; title: string; account: string; selected: boolean; isImport: boolean }[];
  canApprove: boolean;
  blockedReason: string | null;
  approveLabel: string;
  gone: boolean;
}

/** Opens the approval sheet with nothing selected. */
export function openApproval(requestId: string): ApprovalState {
  return core("approval.open", { requestId });
}

export function reduceApproval(o: KeyvaultOverview, state: ApprovalState, action: ApprovalAction): ApprovalState {
  return core("approval.reduce", { overview: o, state, action });
}

export function approvalView(o: KeyvaultOverview, state: ApprovalState): ApprovalView {
  return core("approval.view", { overview: o, state });
}

/** The items Approve sends (`null`: everything asked), or undefined while it cannot. */
export function approvalItems(o: KeyvaultOverview, state: ApprovalState): string[] | null | undefined {
  const cmd = core<{ items: string[] | null } | null>("approval.approveCommand", { overview: o, state });
  return cmd ? cmd.items : undefined;
}

/* ---- The browser (sidebar, lists, site detail) and the page chrome ------- */

export type KvCategory = "all" | "waiting" | "access" | "recent";
export type KvSelection = { kind: "category"; category: KvCategory } | { kind: "site"; key: string };

export interface KvSidebar {
  categories: { category: KvCategory; title: string; symbol: string; count: number | null; badge: number | null }[];
  sites: { key: string; title: string; accounts: number; waiting: boolean }[];
}

export interface KvPendingRow {
  id: string;
  caller: string;
  badge: { text: string; tone: "ok" | "warn" | "danger" };
  summary: string;
  wants: string;
  claims: string[];
}

export type KvCommand =
  | { type: "setup" }
  | { type: "unlock" }
  | { type: "set-disabled"; disabled: boolean }
  | { type: "set-unattended"; itemIds: string[]; unattended: boolean }
  | { type: "revoke-grant"; id: string }
  | { type: "remove-rule"; id: string }
  | { type: "release"; target: string }
  | { type: "approve"; requestId: string; items: string[] | null }
  | { type: "deny"; requestId: string };

export interface KvAccessRow {
  kind: "grant" | "rule" | "delivery";
  key: string;
  text: string;
  detail: string;
  actionLabel: string;
  command: KvCommand;
}

export interface KvListView {
  title: string;
  sites: SiteGroup[];
  pending: KvPendingRow[];
  access: KvAccessRow[];
  recent: { decision: Decision; age: string }[];
  emptyText: string | null;
}

export interface KvSiteDetail {
  group: SiteGroup;
  siteSwitch: boolean;
  siteState: "on" | "off" | "mixed";
  siteSwitchEnabled: boolean;
  siteSwitchHelp: string;
}

export interface KvLabels {
  deny: string;
  review: string;
  cancel: string;
  setUp: string;
  unlock: string;
  revokeAll: string;
  confirmNote: string;
  protectionTitle: string;
  accountsTitle: string;
  appLabel: string;
  everyAccount: string;
}

export interface KvPage {
  ready: boolean;
  unavailableTitle: string | null;
  message: string | null;
  canSetup: boolean;
  canUnlock: boolean;
  killSwitchVisible: boolean;
  killSwitchEnabled: boolean;
  disabled: boolean;
  disabledBanner: string | null;
  partialErrors: string[];
  logStatus: string | null;
  logTampered: boolean;
  protection: { label: string; value: string }[];
  revokeAll: boolean;
  hasItems: boolean;
  searchVisible: boolean;
  pendingCount: number;
  killSwitchHelp: string;
  labels: KvLabels;
  /** The setup or unlock form, when one applies. */
  form: KvCredentialForm | null;
}

export type KvFormMode = "setup" | "unlock";

/** Setting up or unlocking: Touch ID (the OS key store) or a passphrase. */
export interface KvCredentialForm {
  mode: KvFormMode;
  method: "touch-id" | "passphrase";
  help: string;
  passphraseLabel: string | null;
  confirmLabel: string | null;
  submitLabel: string;
}

export interface KvPassphraseCheck {
  canSubmit: boolean;
  strength: "weak" | "fair" | "strong" | null;
  hint: string | null;
}

export const kvSidebar = (o: KeyvaultOverview, now: number): KvSidebar => core("keyvault.sidebar", { overview: o, now });
export const kvList = (o: KeyvaultOverview, selection: KvSelection, now: number, query = ""): KvListView =>
  core("keyvault.list", { overview: o, selection, now, query });
export const kvSiteDetail = (o: KeyvaultOverview, key: string, now: number): KvSiteDetail | null =>
  core("keyvault.siteDetail", { overview: o, key, now });
export const kvPage = (o: KeyvaultOverview, now: number): KvPage => core("keyvault.page", { overview: o, now });
export const kvSiteToggle = (group: SiteGroup, on: boolean): KvCommand => core("keyvault.siteToggle", { group, on });
export const kvRecoveryKeyText = (key: string): string => core("keyvault.recoveryKeyText", { key });
export const kvCredentialForm = (o: KeyvaultOverview): KvCredentialForm | null => core("keyvault.credentialForm", { overview: o });
/** Measures the fields in the wasm core; nothing is kept. */
export const kvPassphraseCheck = (mode: KvFormMode, passphrase: string, confirm: string): KvPassphraseCheck =>
  core("keyvault.passphraseCheck", { mode, passphrase, confirm });
