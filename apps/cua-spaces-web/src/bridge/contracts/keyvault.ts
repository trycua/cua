// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Keyvault wire and view-model types.
 *
 * Hand-written mirror of the Rust source of truth, as of this commit:
 *  - wire: `libs/cua/crates/cua-spaces-app-core/src/keyvault/wire.rs`
 *    (the broker's serde types, snake_case; the overview is camelCase);
 *  - views: `keyvault/view.rs` (`KeyvaultPage`), `keyvault/browse.rs`
 *    (`KvSidebar`, `KvListView`), `keyvault/vault.rs` (`VaultView`: the
 *    list grouped by app), `keyvault/credential.rs`.
 *
 * The Tauri app's `src/native/keyvault.ts` mirrors an older item shape (one
 * item per site); the core moved to one item per secret. No secret values
 * are here: the broker's ListItems is the redacted view.
 */

/* ---- Wire (wire.rs) ------------------------------------------------------ */

export interface KvItemPolicy {
  allowed_targets: string[];
  ttl_secs: number;
  /** Unlocked: allowed unattended. Locked (false): asks for every use. */
  unattended: boolean;
}

/** `cookie`, `local_storage`, `password` or `file`. */
export type KvItemKind = "cookie" | "local_storage" | "password" | "file" | (string & {});

/** One secret: a cookie, a localStorage value, a password or a file. */
export interface KvItem {
  id: string;
  kind: KvItemKind;
  /** `chrome`, `safari`, `slack`. */
  provider_id: string;
  /** "Google Chrome", "Slack". */
  app_display: string;
  /** The site (registrable domain), for web secrets. */
  domain?: string;
  /** Cookie name, storage key, username or file name (empty while names are hidden). */
  key: string;
  /** A file's path. */
  path?: string;
  source: string;
  session: boolean;
  expires_ms?: number;
  bytes: number;
  blob?: string;
  identity_provider: boolean;
  policy: KvItemPolicy;
  created_ms: number;
  updated_ms: number;
  rev: number;
  record_digest: string;
}

export type KvSigning =
  | { kind: "signed"; team_id: string; identifier: string; cdhash: string }
  | { kind: "ad_hoc"; identifier: string; cdhash: string }
  | { kind: "unsigned" }
  | { kind: "unknown" };

export interface KvCaller {
  pid: number;
  uid: number;
  path?: string | null;
  signing: KvSigning;
  first_party: boolean;
  os_verified: boolean;
  launched_by?: string | null;
  verified_name?: string | null;
}

export type KvSelector =
  | { kind: "item"; id: string }
  | { kind: "site"; app: string; site: string }
  | { kind: "app"; app: string }
  | { kind: "login"; site: string };

export interface KvAccessRequest {
  selectors: KvSelector[];
  targets: string[];
  actions: string[];
  duration_secs?: number | null;
  uses?: number | null;
  reason: string;
  claimed_name?: string | null;
  agent?: string | null;
}

export interface KvPending {
  id: string;
  caller: KvCaller;
  caller_fp: string;
  caller_display: string;
  request: KvAccessRequest;
  items: KvItem[];
  needs_import: KvSelector[];
  created_ms: number;
}

export interface KvGrant {
  id: string;
  request_id: string;
  caller_fp: string;
  caller_display: string;
  items: string[];
  targets: string[];
  actions: string[];
  created_ms: number;
  not_after_ms: number;
  uses_left?: number | null;
  revoked: boolean;
  agent?: string | null;
}

export interface KvRule {
  id: string;
  items: string[];
  targets: string[];
  callers: { fp: string; display: string }[];
  created_ms: number;
  not_after_ms: number;
  enabled: boolean;
  note: string;
}

export interface KvDelivery {
  import_id: string;
  target: string;
  provider_id: string;
  items: string[];
  caller_fp: string;
  delivered_ms: number;
  expires_ms: number;
  wiped: boolean;
}

export interface KvAuditEntry {
  seq: number;
  ts_ms: number;
  kind: string;
  actor: string;
  caller_fp: string;
  item?: string;
  target?: string;
  decision: string;
  detail?: string;
  authless?: boolean;
}

export interface KvStatus {
  version: string;
  initialized: boolean;
  unlocked: boolean;
  disabled: boolean;
  caller_first_party: boolean;
  caller_display: string;
  items: number;
  pending: number;
  unlock_policy?: "auto" | "presence" | (string & {});
  auto_wipe?: boolean;
  os_protector_available: boolean;
  passphrase_available: boolean;
  /** Protectors that can unlock this vault now (`macos-keychain`, `passphrase`). */
  unlock_protectors: string[];
  browse_until_ms?: number;
  skip_unlock_prompt?: boolean;
  reset_notice?: string;
}

export interface KvVerification {
  ok: boolean;
  entries: number;
  unauthenticated: number;
  tampered_line?: number;
  reason?: string;
}

/** `ready`, or why not. */
export type KvAvailability =
  | "ready"
  | "not_running"
  | "impostor"
  | "connect"
  | "no_vault"
  | "locked"
  | "not_first_party"
  | "unsupported"
  | "error"
  | (string & {});

/** Everything the Keyvault page shows, in one read (`KeyvaultOverview`). */
export interface KeyvaultOverview {
  availability: KvAvailability;
  message?: string;
  status?: KvStatus;
  serverVerified: boolean;
  items: KvItem[];
  /** Domains and keys are present in `items` (the browse window is open). */
  namesVisible: boolean;
  itemsTotal: number;
  pending: KvPending[];
  grants: KvGrant[];
  rules: KvRule[];
  deliveries: KvDelivery[];
  audit: KvAuditEntry[];
  auditVerification?: KvVerification;
  partialErrors: string[];
  /** Delivered copies (import ids) the user hid from the notch (SwiftUI host); they stay live. */
  dismissed?: string[];
}

/* ---- Views (view.rs, browse.rs, credential.rs) ---------------------------- */

export type Tone = "ok" | "warn" | "danger";
export type Tri = "on" | "off" | "mixed";
export type KvLock = "locked" | "unlocked" | "mixed";

export interface SigningBadge {
  text: string;
  tone: Tone;
}

/** A page action: one broker request (`KvCommand`). */
export type KvCommand =
  | { type: "setup" }
  | { type: "unlock" }
  | { type: "set-disabled"; disabled: boolean }
  | { type: "set-unattended"; itemIds: string[]; unattended: boolean }
  | { type: "set-locked"; itemIds: string[]; locked: boolean }
  | { type: "delete-items"; itemIds: string[] }
  | { type: "set-skip-unlock-prompt"; on: boolean }
  | { type: "browse" }
  | { type: "end-browse" }
  | { type: "revoke-grant"; id: string }
  | { type: "remove-rule"; id: string }
  | { type: "set-auto-wipe"; on: boolean }
  | { type: "release"; target: string }
  | { type: "approve"; requestId: string; items: string[] | null }
  | { type: "deny"; requestId: string };

export interface PendingRow {
  id: string;
  caller: string;
  badge: SigningBadge;
  summary: string;
  wants: string;
  claims: string[];
}

export interface AccessRow {
  kind: "grant" | "rule" | "delivery";
  key: string;
  text: string;
  detail: string;
  actionLabel: string;
  command: KvCommand;
  imports: string[];
}

export interface Decision {
  entry: KvAuditEntry;
  verb: string;
  tone: "ok" | "deny" | "info";
  what: string;
}

export type KvCategory = "all" | "waiting" | "access" | "recent";
export type KvSelection = { kind: "category"; category: KvCategory } | { kind: "app"; key: string };

export interface KvSidebar {
  categories: { category: KvCategory; title: string; symbol: string; count: number | null; badge: number | null }[];
  apps: { key: string; title: string; items: number; waiting: boolean }[];
}

/** The pane for a selection other than the vault list (`vault` true: draw `VaultView`). */
export interface KvListView {
  title: string;
  vault: boolean;
  pending: PendingRow[];
  access: AccessRow[];
  recent: { decision: Decision; age: string }[];
  emptyText: string | null;
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
}

export interface KvCredentialForm {
  mode: "setup" | "unlock";
  method: "touch-id" | "passphrase" | (string & {});
  help: string;
  passphraseLabel: string | null;
  confirmLabel: string | null;
  submitLabel: string;
}

/** The page chrome (`KeyvaultPage`). */
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
  skipUnlockPrompt: boolean;
  resetNotice: string | null;
  pendingCount: number;
  killSwitchHelp: string;
  labels: KvLabels;
  form: KvCredentialForm | null;
}

/* ---- The vault list, grouped by app (vault.rs) --------------------------- */

/** The list's own state (`VaultState`). */
export interface VaultState {
  query: string;
  selected: string[];
  /** Open groups (their keys). */
  expanded: string[];
  /** Narrowed to one app (its provider id). */
  app?: string | null;
}

/** An input to the list's reducer (`vault::reduce`). */
export type VaultAction =
  | { type: "query"; text: string }
  | { type: "toggle"; id: string }
  | { type: "toggle-group"; key: string }
  | { type: "select-all" }
  | { type: "clear" }
  | { type: "toggle-open"; key: string };

export interface VaultRow {
  id: string;
  kind: "cookie" | "local_storage" | "password" | "file";
  kindLabel: string;
  symbol: string;
  title: string;
  subtitle: string;
  updated: string;
  locked: boolean;
  lockSymbol: string;
  lockHelp: string;
  identityProvider: boolean;
  selected: boolean;
}

interface VaultGroupBase {
  key: string;
  count: number;
  selected: Tri;
  lock: KvLock;
  /** What a click on the lock unlocks (locked, not identity providers). */
  unlockIds: string[];
  /** What a click on the lock locks. */
  lockIds: string[];
  open: boolean;
  rows: VaultRow[];
}

export interface VaultSite extends VaultGroupBase {
  site: string;
  /** "12 cookies, 2 storage values". */
  counts: string;
  updated: string;
}

export type VaultFiles = VaultGroupBase;

export interface VaultApp extends Omit<VaultGroupBase, "rows"> {
  providerId: string;
  name: string;
  /** "212 items, 14 unlocked". */
  summary: string;
  updated: string;
  sites: VaultSite[];
  files: VaultFiles | null;
}

export interface VaultSelection {
  count: number;
  ids: string[];
  title: string;
  canUnlock: boolean;
  canLock: boolean;
  alwaysAsk: number;
  unlockIds: string[];
  lockIds: string[];
}

export interface VaultView {
  apps: VaultApp[];
  shown: number;
  total: number;
  emptyText: string | null;
  namesHidden: boolean;
  hiddenNote: string | null;
  showNamesLabel: string;
  searchPrompt: string;
  selection: VaultSelection;
  canSelectAll: boolean;
}

/** What the Keyvault holds for one app that a teleport can send (`vault::KvVaultSource`;
 * saved passwords are listed apart and sent only when ticked). */
export interface VaultSource {
  count: number;
  /** When the newest was saved, Unix ms (0: none). */
  newestMs: number;
  ids: string[];
  passwordIds: string[];
}

/* ---- The approval sheet (approval.rs) ------------------------------------- */

/** The sheet's state: nothing is ticked until the user ticks (`ApprovalState`). */
export interface ApprovalState {
  requestId: string;
  selected: string[];
}

export type ApprovalAction = { type: "toggle"; key: string } | { type: "select-all" } | { type: "clear" };

export interface ApprovalRow {
  key: string;
  title: string;
  /** "3 cookies, 1 password", or "Not saved yet". */
  account: string;
  providerId: string;
  items: number;
  itemIds: string[];
  selected: boolean;
  isImport: boolean;
}

/** The sheet as drawn (`ApprovalView`). */
export interface ApprovalView {
  requestId: string;
  title: string;
  caller: string;
  badge: SigningBadge;
  targets: string;
  wants: string;
  summary: string;
  claims: string[];
  rows: ApprovalRow[];
  canApprove: boolean;
  blockedReason: string | null;
  approveLabel: string;
  /** The request is gone (answered elsewhere). */
  gone: boolean;
}
