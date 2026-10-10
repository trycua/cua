// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "Teleport an app": the picker, its grid and the review, as the app core
 * models them. Hand-written mirrors of
 * `libs/cua/crates/cua-spaces-app-core/src/teleport/{flow,grid,windows,review}.rs`
 * (camelCase) and `keyvault/wire.rs` (`KvInventory`, snake_case). The core
 * decides every step; these only carry its state and views.
 */

export type Capability = "full" | "install_only" | "unsupported";
export type TeleportMove = "app_only" | "app_with_files" | "app_with_state";
export type SensitiveGroup = "sign_ins" | "passwords" | "history";

/** One app (`flow::CatalogEntry`). `json` is the SDK's own JSON for it. */
export interface CatalogEntry {
  id: string;
  name: string;
  hostPath: string | null;
  hostAppId: string | null;
  version: string | null;
  capability: Capability;
  reason: string | null;
  moves: TeleportMove[];
  providerId: string | null;
  sensitiveGroups?: SensitiveGroup[];
  installSource: string | null;
  installId: string | null;
  installVersion: string | null;
  launchBin: string | null;
  lastUsedMs: number | null;
  json: string;
}

export type ConsentKind = "install" | "file" | "folder" | "state" | "secret";

/** One line of the review (`flow::ConsentItem`). */
export interface ConsentItem {
  kind: ConsentKind;
  key: string;
  label: string;
  detail: string;
  bytes: number;
  sensitive: boolean;
}

export interface PlanStep {
  kind: string;
  summary: string;
}

/** What a teleport will do (`flow::Plan`). */
export interface TeleportPlan {
  app: CatalogEntry;
  spaceId: string;
  moves: TeleportMove;
  steps: PlanStep[];
  consent: ConsentItem[];
  sensitive: boolean;
  totalBytes: number;
  warnings: string[];
  relayUnsealed?: boolean;
  json: string;
}

export type RunPhase = "started" | "progress" | "finished" | "failed" | "done";

export interface RunEvent {
  step: number;
  steps: number;
  kind: string;
  phase: RunPhase;
  detail: string;
  doneBytes: number;
  totalBytes: number;
}

export interface RunReport {
  appId: string;
  installed: string[];
  sent: string[];
  imported: string[];
  skipped: string[];
  launched: boolean;
}

/** The consent a run carries (`flow::Consent`, from `flow.consent`). */
export interface TeleportConsent {
  approved: boolean;
  acknowledgeSensitive: boolean;
  saveToKeyvault?: boolean;
  acknowledgeRelayPlaintext?: boolean;
  cookieDomains?: string[] | null;
  exclude?: string[];
  fromVault?: string[] | null;
  includePasswords?: boolean;
}

export type PickerStep = "loading" | "pick" | "options" | "planning" | "consent" | "running" | "done" | "error";

/** The picker's state (`flow::PickerState`). The core owns it; the page
 * only passes it back. */
export interface PickerState {
  step: PickerStep;
  spaceName: string;
  entries: CatalogEntry[] | null;
  query: string;
  selectedId: string | null;
  entry: CatalogEntry | null;
  move: TeleportMove | null;
  files: string[];
  sensitive: SensitiveGroup[];
  plan: TeleportPlan | null;
  acknowledged: boolean;
  saveToKeyvault: boolean;
  acknowledgedRelayPlaintext: boolean;
  choice: unknown;
  events: RunEvent[];
  report: RunReport | null;
  error: string | null;
  installPrompt: { message: string } | null;
  errorBack: PickerStep;
}

/** A browser's sites with counts, never values (`keyvault::wire::KvInventory`). */
export interface KvDomainCount {
  domain: string;
  cookies?: number;
  session_cookies?: number;
  local_storage?: number;
  passwords?: number;
  signin?: boolean;
  identity_provider?: boolean;
  unavailable?: number;
  unavailable_reason?: string;
}

export interface KvInventory {
  provider_id: string;
  app_display: string;
  domains: KvDomainCount[];
  notes?: string[];
}

/** An input to the picker (`flow::PickerEvent`). */
export type PickerEvent =
  | { type: "loaded"; entries: CatalogEntry[] }
  | { type: "failed"; message: string; causeTexts?: string[]; causeInstalled?: boolean }
  | { type: "query"; query: string }
  | { type: "select"; id: string }
  | { type: "choose"; id?: string | null }
  | { type: "preselect"; entry: CatalogEntry; files?: string[] }
  | { type: "move"; move: TeleportMove }
  | { type: "files"; files: string[] }
  | { type: "remove-file"; path: string }
  | { type: "sensitive"; group: SensitiveGroup; value: boolean }
  | { type: "plan" }
  | { type: "planned"; plan: TeleportPlan }
  | { type: "acknowledge"; value: boolean }
  | { type: "save-to-keyvault"; value: boolean }
  | { type: "acknowledge-relay-plaintext"; value: boolean }
  | { type: "domains-loaded"; inventory: KvInventory; remembered?: string[] | null }
  | { type: "domains-failed" }
  | { type: "toggle-domain"; domain: string }
  | { type: "select-shown-domains"; value: boolean }
  | { type: "domain-query"; text: string }
  | { type: "toggle-item"; key: string }
  | { type: "toggle-passwords"; value: boolean }
  | { type: "send-from"; source: "live" | "vault" }
  | { type: "vault-items"; count: number; newestMs: number; nowMs: number; selected: string[]; passwordIds: string[] }
  | { type: "vault-selection"; selected: string[] }
  | { type: "confirm" }
  | { type: "progress"; event: RunEvent }
  | { type: "finished"; report: RunReport }
  | { type: "back" };

/** "Recent", "Apps", "Not available" (`flow::EntrySection`). */
export interface EntrySection {
  title: string;
  entries: CatalogEntry[];
}

/** An opt-in of the signed-in state move (`flow::SensitiveOption`). */
export interface SensitiveOption {
  group: SensitiveGroup;
  label: string;
  detail: string;
  checked: boolean;
}

/** One site of the review (`review::ReviewDomain`). */
export interface ReviewDomain {
  domain: string;
  counts: string;
  count: number;
  signin: boolean;
  identityProvider: boolean;
  selected: boolean;
  selectable: boolean;
  unavailable: number;
  unavailableNote: string;
}

/** A consent line the user can turn off (`review::ReviewToggle`). */
export interface ReviewToggle {
  key: string;
  label: string;
  detail: string;
  bytes: number;
  sensitive: boolean;
  selected: boolean;
}

/** The review as drawn (`flow::ReviewView`). */
export interface ReviewView {
  title: string;
  items: ConsentItem[];
  steps: PlanStep[];
  needsAcknowledgement: boolean;
  acknowledged: boolean;
  offersSaveToKeyvault: boolean;
  saveToKeyvault: boolean;
  needsRelayPlaintextAcknowledgement: boolean;
  acknowledgedRelayPlaintext: boolean;
  canConfirm: boolean;
  leavesText: string | null;
  warnings: string[];
  toggles: ReviewToggle[];
  offersDomains: boolean;
  needsDomains: boolean;
  domains: ReviewDomain[];
  domainSummary: string;
  domainQuery: string;
  selectedDomains: string[];
  source: "live" | "vault";
  offersVault: boolean;
  vaultLabel: string;
  liveLabel: string;
  offersPasswords: boolean;
  includePasswords: boolean;
  passwordsLabel: string;
  sourceNote: string;
}

/** Everything a picker frame draws, from the core (`picker_frame` in parity.rs). */
export interface PickerFrame {
  sections: EntrySection[];
  review: ReviewView | null;
  canPlan: boolean;
  /** 0..1 */
  progress: number;
  /** What the run is doing, in words ("Uploading 12 / 80 MB"); null unless running (`flow::status`). */
  status: string | null;
  sensitive: SensitiveOption[];
  planSensitive: SensitiveGroup[];
}

/* ---- The grid (`teleport::grid`, `teleport::windows`) ---------------------- */

/** One of this machine's windows. */
export interface OpenWindow {
  windowId: number;
  appId: string;
  appName: string;
  windowTitle: string;
  supported: boolean;
  bundlePath?: string | null;
}

/** One of the Space's windows (`model::RemoteWindow`; also the Space detail's windows). */
export interface RemoteWindow {
  id: string;
  appName: string;
  title: string;
  visible: boolean;
  appId: string;
  targetEpoch: number;
  widthPx?: number | null;
  heightPx?: number | null;
  pid?: number | null;
}

export type PickerTileIcon =
  | { kind: "host"; path: string }
  | { kind: "guest"; appName: string; appId: string; pid: number }
  | { kind: "none" };

export type PickerTileThumbnail =
  | { kind: "host-window"; windowId: number }
  | { kind: "guest-window"; windowId: string; epoch: number }
  | { kind: "none" };

export interface PickerTile {
  id: string;
  title: string;
  /** The tooltip: what the app can take, why not, or the full title. */
  help: string;
  disabled: boolean;
  selected: boolean;
  icon: PickerTileIcon;
  thumbnail: PickerTileThumbnail;
}

export interface PickerTileSection {
  title: string;
  tiles: PickerTile[];
}

export interface PickerGrid {
  sections: PickerTileSection[];
  emptyText: string | null;
}

export type PickerGridTab = "apps" | "windows" | "space";

export interface PickerGridTabItem {
  tab: PickerGridTab;
  label: string;
}

export interface PickerGridPrimary {
  label: string;
  enabled: boolean;
}
