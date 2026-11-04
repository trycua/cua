// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * New Space and "Connect a cloud": the app core's wizard and sheet records
 * (`cua-spaces-app-core::wizard`, `::cloud_connect`) and what the hosts
 * report for them (`local_status`, `gpu_support`,
 * `cloud_status`, `cloud_test`, `cloud_connect`).
 *
 * Hand-written mirror of the Tauri app's `components/desktop/
 * NewSpaceWizard.tsx`, `model/cloudConnect.ts` and `native/cloud.ts`. Rust is
 * the source of truth; these are its JSON shapes.
 */

import type { SandboxImage } from "./onboarding";
import type { Runtime, SpaceKind, SpaceOs } from "./spaces";
import type { Experiments } from "./volume";

/* ---- What the host knows (WizardEnv) ------------------------------------- */

/** Where a new Space runs (`model::Location`). */
export type WizardLocation = "local" | "yours" | "host";

/** Settings, Experiments (`experiments::Experiments`): one shape, defined with Volume. */
export type { Experiments } from "./volume";

/** Space on the volume a local engine writes to. */
export interface StorageVolume {
  availableBytes: number;
  totalBytes: number;
  name: string;
}

/** Where local Spaces are written and what is already pulled. */
export interface LocalStorage {
  reserveBytes: number;
  linuxRuntimeDownloadBytes?: number;
  lume: StorageVolume | null;
  qemu: StorageVolume | null;
  container: StorageVolume | null;
  pulled: string[];
}

/** Can this machine run Spaces (`local_status`). */
export interface LocalStatus {
  available: boolean;
  /** Ready backends from `cua runtime doctor` (`docker`, `lume`, ...). */
  backends: string[];
  containerImage?: string;
  macosImage?: string | null;
  error: string | null;
  /** What the host can do about `error` (Windows: install or start Docker Desktop). */
  fix?: RuntimeFix | null;
  hostArch?: string;
  storage?: LocalStorage | null;
}

/** `spaces.fixRuntime`'s actions. */
export type RuntimeFixAction = "install-docker" | "start-docker" | "restart-app";

/** A step the host offers next to "no local runtime" (`LocalStatus.fix`). */
export interface RuntimeFix {
  /** What the button runs; null while something runs, or when only the person can act. */
  action: RuntimeFixAction | null;
  label: string;
  note: string | null;
  /** Something runs now: read the options again until it settles. */
  busy: boolean;
  /** Terms the action accepts, linked beside the note. */
  termsUrl: string | null;
}

/** A runtime's GPU option (`gpu_support`). */
export interface GpuChoice {
  runtime: string;
  id: string;
  label: string;
  experimental: boolean;
  supported: boolean;
  reason?: string | null;
  learnMore?: string | null;
}

/** What one connected cloud runs for an image family. */
export interface CloudOffer {
  image: string;
  kind: string;
  supported: boolean;
  reason?: string;
  machineType?: string;
  usdPerHour?: number;
}

/** A connected cloud (`wizard::ConnectedCloud`). */
export interface ConnectedCloud {
  name: string;
  title: string;
  label: string;
  isDefault: boolean;
  ttlHours: number;
  offers: CloudOffer[];
}

/** One of your machines that provides Spaces (`wizard::SpaceHost`). */
export interface WizardHost {
  id: string;
  name: string;
  via: string;
  online: boolean;
  os: string;
  limits: { resource: string; used: number; limit: number; reason: string }[];
}

/** What the wizard knows about this machine and account (`wizard::WizardEnv`). */
export interface WizardEnv {
  defaultLocation: WizardLocation;
  localAvailable: boolean;
  localReason?: string | null;
  localBackends?: string[] | null;
  /** Why each local backend that is not ready is not (the runtime doctor's detail). */
  localDetails?: Record<string, string> | null;
  maxCpus: number;
  hostArch?: string | null;
  storage?: LocalStorage | null;
  clouds?: ConnectedCloud[];
  hosts?: WizardHost[];
  experiments?: Experiments;
  gpus?: GpuChoice[] | null;
  /** Picks the name an unnamed Space gets (`wizard::generated_space_name`):
   * the wasm core has no randomness of its own, so the page gives one. */
  nameSeed?: number | null;
}

/** `spaces.createOptions`: what New Space needs from the host beyond Spaces,
 * machines, settings and the session. Each part is null when the host
 * could not say. */
export interface NewSpaceOptions {
  local: LocalStatus | null;
  gpus: GpuChoice[] | null;
  experiments: Experiments;
  /** Upper bound for the CPU slider; null: the page's own count. */
  maxCpus: number | null;
  /** The host's own wizard env, when it has one (the SwiftUI app's
   * `AppModel.newSpaceEnv`, which its native sheet opens with): the wizard
   * runs on it as it is, with your machines and clouds as the host read
   * them. */
  env?: WizardEnv | null;
  /** macOS VMs running on this machine now, Spaces or not (the SwiftUI
   * host asks Lume): Apple's license allows two per Mac. Null or absent
   * when the host doesn't say. */
  macosVmsRunning?: number | null;
  /** The host is still starting (the macOS Keychain, the daemon): this is
   * what it knows so far, without this machine's runtimes or storage. The
   * page asks again once the host is ready. */
  pending?: boolean;
}

/* ---- The wizard (state, actions, view) ----------------------------------- */

/** The core's wizard state; the page keeps it and never reads into it
 * except for the address form's text. */
export type WizardState = Record<string, unknown> & {
  address?: { url: string; token: string; name: string };
};

export type WizardAction = { type: string } & Record<string, unknown>;

/** A Resources slider whose value can be typed (`wizard::ResourceField`). */
export type ResourceField = "cpus" | "memory" | "disk";

/** A slider's value being typed (`wizard::ResourceEditView`). */
export interface ResourceEdit {
  field: ResourceField;
  text: string;
  error: string | null;
}

export interface WizardTile {
  id: string;
  title: string;
  detail: string;
  pressed: boolean;
  enabled: boolean;
}

/** A runtime setting New Space offers to switch (`wizard::RuntimeSwitch`). */
export interface RuntimeSwitch {
  /** The `cua config` setting: `runtime.lume` or `runtime.linux`. */
  setting: string;
  /** The value to set: `builtin`. */
  value: string;
  /** The button: "Use built-in Lume". */
  label: string;
  /** One line on what that does (the button's tooltip). */
  detail: string;
}

/** One entry of the "Run on" menu (`wizard::PlacementOption`). */
export interface PlacementOption {
  /** What choosing it sets as `on`: `local`, `host:<id>`, `aws`, ... */
  id: string;
  label: string;
  /** `this-mac`, `hosts`, `clouds`. */
  group: string;
  selected: boolean;
  enabled: boolean;
  detail: string;
}

export interface WizardField {
  id: string;
  label: string;
  placeholder: string | null;
  error: string | null;
  advanced: boolean;
}

export interface WizardFact {
  id: string;
  label: string;
  value: string;
  symbol?: string | null;
  help?: string | null;
}

export interface GpuRow {
  label: string;
  on: boolean;
  enabled: boolean;
  reason: string | null;
  learnMoreLabel: string;
  learnMoreUrl: string | null;
}

export interface ImageFieldView {
  text: string;
  open: boolean;
  groups: {
    id: string;
    label: string;
    rows: { ref: string; label: string; highlighted: boolean; selected: boolean }[];
  }[];
  error: string | null;
  custom: boolean;
}

/** What Create sends (`wizard::CreatePlan`). */
export interface CreatePlan {
  placement: WizardLocation;
  cloud?: string;
  host?: string;
  image: SandboxImage;
  runtime: Runtime;
  name?: string;
  cpus?: number;
  memoryMb?: number;
  diskGb?: number;
  openWhenReady: boolean;
  openDesktop: boolean;
  gpu?: string;
}

/** The SDK call a plan makes (`wizard::CreateSpaceArgs`). */
export interface CreateSpaceArgs {
  image: string;
  on: string;
  kind: SpaceKind;
  runtime: Runtime;
  name?: string;
  cpus?: number;
  memoryMb?: number;
  diskGb?: number;
  spacesd: boolean;
  gpu?: string;
}

export interface WizardView {
  mode: "create" | "address";
  step: number;
  steps: { label: string; state: "done" | "current" | "todo" }[];
  title: string;
  osTiles: WizardTile[];
  imageField: ImageFieldView;
  image: SandboxImage & { os: SpaceOs };
  placements: PlacementOption[];
  placementId: string;
  cloud: string | null;
  host: string | null;
  placementError: string | null;
  /** With `placementError` when a cloud of yours cannot run the image: the
   * line to copy for an agent that runs it there by hand. */
  placementPrompt?: string | null;
  placementHint?: string | null;
  /** A setting that lets This Mac run it, offered beside `placementError`
   * ("Use built-in Lume"), when the person chose only this Mac's own
   * runtime and it is missing (`wizard::RuntimeSwitch`). */
  runtimeSwitch?: RuntimeSwitch | null;
  advanced: boolean;
  kindTiles: WizardTile[];
  runtimes: { value: Runtime; label: string }[];
  runtime: Runtime;
  runtimeEnabled: boolean;
  cpus: number;
  minCpus: number;
  maxCpus: number;
  minMemoryGb: number;
  maxMemoryGb: number;
  cpusText: string;
  memoryGb: number;
  memoryText: string;
  diskEditable: boolean;
  diskGb: number;
  minDiskGb: number;
  maxDiskGb: number;
  diskText: string;
  diskNote: string | null;
  diskResetLabel: string | null;
  diskHelp: string | null;
  price: string | null;
  resourceFacts: WizardFact[];
  resourcesError: string | null;
  gpu: GpuRow | null;
  /** A slider's value being typed (its number was clicked), with why it
   * cannot be used (the slider's limits). */
  resourceEdit?: ResourceEdit | null;
  name: string;
  /** The name the Space gets: the one typed, else the generated one. */
  spaceName?: string;
  nameInvalid: boolean;
  nameError: string | null;
  openWhenReady: boolean;
  streamNote: string | null;
  summary: { label: string; value: string }[];
  canContinue: boolean;
  showBack: boolean;
  primaryLabel: string;
  plan: CreatePlan;
  address: {
    valid: boolean;
    showInvalid: boolean;
    canSubmit: boolean;
    submitLabel: string;
    error: string | null;
    submit: { url: string; token: string | null; name: string | null } | null;
  };
  fields: WizardField[];
  labels: { cancel: string; back: string; advanced: string };
}

/* ---- Connect a cloud ------------------------------------------------------ */

/** What the sheet reads (`cloud_sheet::CloudSheetInput`). */
export interface CloudSheetInput {
  providers: { name: string; found: boolean; region: string; environment: string; budget: number | null; maxSize: string }[];
}

export type CloudSheetState = Record<string, unknown>;

export interface CloudSheetAnswer {
  status: string;
  open: string;
  paste: string;
  why: string;
  failedCheck: string;
}

export type CloudSheetAction =
  | { type: "choose"; id: string }
  | { type: "back" }
  | { type: "set-place"; text: string }
  | { type: "set-budget"; text: string }
  | { type: "set-size"; text: string }
  | { type: "set-account-id"; text: string }
  | { type: "set-token-entered"; entered: boolean }
  | { type: "press" }
  | ({ type: "answered" } & CloudSheetAnswer)
  | { type: "failed"; error: string };

/**
 * The `cloud_connect` call the sheet asks for: names and numbers only. The
 * one secret is Hetzner's API token: `uses_token` is the core's request to
 * attach it, and `token` is what the shell adds for that one call (never
 * stored, logged or sent to the core).
 */
export interface CloudTarget {
  provider: string;
  profile?: string;
  region?: string;
  project?: string;
  environment?: string;
  /** `oidc` for AWS through Cua's issuer. */
  auth?: string;
  budget_usd_month?: number;
  /** The largest machine, `4x16`. */
  max_size?: string;
  /** AWS: the account the trust stack is in. */
  account_id?: string;
  /** Run the check that proves the link. */
  check?: boolean;
  uses_token?: boolean;
  token?: string;
}

export interface CloudSheetField {
  id: "place" | "budget" | "size" | "account" | "token";
  label: string;
  kind: "select" | "text" | "money" | "secret";
  value: string;
  placeholder: string;
  options: { value: string; label: string }[];
}

export interface CloudSheetView {
  stage: "picker" | "cloud";
  title: string;
  logo: string;
  intro: string;
  rows: { id: string; name: string; logo: string; auth: string; note: string }[];
  fields: CloudSheetField[];
  trust: { text: string; openUrl: string; openLabel: string; paste: string; copyLabel: string } | null;
  note: string;
  detailsLabel: string;
  detailsUrl: string;
  promptTitle: string;
  prompt: string;
  copyLabel: string;
  primaryLabel: string;
  primaryEnabled: boolean;
  backLabel: string;
  cancelLabel: string;
  footnote: string;
  error: string | null;
  done: boolean;
  request: { provider: string; auth?: string; region?: string; environment?: string; budget_usd_month: number; max_size?: string; account_id?: string; check?: boolean; uses_token?: boolean } | null;
}

/* ---- The SDK's cloud tools (snake_case wire records) --------------------- */

export interface CloudKindWire {
  image: string;
  kind: string;
  supported: boolean;
  reason?: string;
  machine_type?: string;
  usd_per_hour?: number;
}

/** One provider as `cloud_status` lists it. */
export interface CloudProviderWire {
  name: string;
  title: string;
  tier: string;
  connected: boolean;
  default?: boolean;
  credentials?: { found: boolean; source?: string };
  account?: string;
  profile?: string;
  region?: string;
  zone?: string;
  project?: string;
  environment?: string;
  label?: string;
  ttl_hours?: number;
  kinds?: CloudKindWire[];
  /** `oidc`, `sealed` or `cli`; empty when not connected. */
  auth?: string;
  /** `connected`, `waiting-for-approval`, `failed`, `not-connected`. */
  status?: string;
  budget_usd_month?: number;
  /** The largest machine, `4x16`. */
  max_size?: string;
  spent_month?: number;
  running?: number;
  /** What waits for a person: `connection`, `limits` or empty. */
  approval?: string;
  /** Who can open a sealed token, when the host knows. */
  holders?: string[];
  /** The agent that set the cloud up, while its approval waits ("Claude Code on This Mac"). */
  approval_agent?: string;
  /** The budget an agent asked for, while it waits. */
  pending_budget_usd_month?: number | null;
  /** The size an agent asked for, while it waits (`8x32`). */
  pending_max_size?: string;
  /** This caller (a person) may approve or deny what waits (`cloud_approve`). */
  can_approve?: boolean;
  /** `oidc` when a CLI sign-in link could link without a stored key. */
  suggest_auth?: string;
  /** What is wrong with the link and what to do, one sentence. */
  issue?: string;
}

/** What Cua created in a cloud. */
export interface CloudResourceWire {
  provider: string;
  id: string;
  type: string;
  sandbox?: string;
  state?: string;
  started_by?: string;
  cost_usd?: number;
}

/** `cloud_status`. */
export interface CloudStatusWire {
  default_on?: string;
  providers: CloudProviderWire[];
  resources?: CloudResourceWire[];
  /** The host can approve what an agent set up (it has the approve tool). */
  can_approve?: boolean;
}

export interface CloudCheckWire {
  name: string;
  ok: boolean;
  detail?: string;
}

/** `cloud_test`: creates nothing. */
export interface CloudTestWire {
  provider: string;
  ok: boolean;
  account?: string;
  checks: CloudCheckWire[];
}

/** `cloud_connect`: the provider as now linked, and what to do next. */
export type CloudConnectWire = CloudProviderWire & {
  checks?: CloudCheckWire[];
  next?: { run?: string; open?: string; paste?: string; why?: string };
  verify?: string;
};

/** `cloud_disconnect`. */
export interface CloudDisconnectedWire {
  provider: string;
  disconnected: boolean;
}
