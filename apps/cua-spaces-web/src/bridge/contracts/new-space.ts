// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * New Space and "Connect a cloud": the app core's wizard and sheet records
 * (`cua-spaces-app-core::wizard`, `::cloud_connect`) and what the hosts
 * report for them (`local_status`, `gpu_support`, `cloud_pricing`,
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
export type WizardLocation = "cloud" | "local" | "yours" | "host";

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
  hostArch?: string;
  storage?: LocalStorage | null;
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

/** This account's Cua Cloud rates (`cloud_pricing`). */
export interface CloudPricing {
  vcpuHourUsd: number;
  memoryGibHourUsd: number;
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
  cloudAvailable: boolean;
  localAvailable: boolean;
  localReason?: string | null;
  localBackends?: string[] | null;
  maxCpus: number;
  hostArch?: string | null;
  storage?: LocalStorage | null;
  cloudPricing?: CloudPricing | null;
  clouds?: ConnectedCloud[];
  hosts?: WizardHost[];
  experiments?: Experiments;
  gpus?: GpuChoice[] | null;
}

/** `spaces.createOptions`: what New Space needs from the host beyond Spaces,
 * machines, settings and the session. Each part is null when the host
 * could not say. */
export interface NewSpaceOptions {
  local: LocalStatus | null;
  gpus: GpuChoice[] | null;
  cloudPricing: CloudPricing | null;
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
  name: string;
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

export interface CloudProviderInput {
  name: string;
  title: string;
  connected?: boolean;
  found?: boolean;
  source?: string;
  profile?: string;
  region?: string;
  project?: string;
  environment?: string;
  label?: string;
}

export interface CloudConnectInput {
  providers: CloudProviderInput[];
}

export type CloudConnectState = Record<string, unknown>;

export type CloudConnectAction =
  | { type: "select"; name: string }
  | { type: "set-value"; text: string }
  | { type: "set-profile"; text: string }
  | { type: "set-make-default"; on: boolean }
  | { type: "test" }
  | { type: "tested"; ok: boolean; account: string; checks: { name: string; ok: boolean; detail: string }[] }
  | { type: "connect" }
  | { type: "connected"; label: string }
  | { type: "failed"; error: string };

/** Where Spaces go in the account (names only; credentials stay with the CLI). */
export interface CloudTarget {
  provider: string;
  profile?: string;
  region?: string;
  project?: string;
  environment?: string;
}

export type CloudConnectRequest =
  | { kind: "test"; target: CloudTarget }
  | { kind: "connect"; target: CloudTarget; make_default: boolean };

export interface CloudField {
  id: string;
  label: string;
  placeholder: string;
  value: string;
}

export interface CloudConnectView {
  title: string;
  rows: { id: string; title: string; detail: string; found: boolean; selected: boolean }[];
  field: CloudField | null;
  profileField: CloudField | null;
  checks: { ok: boolean; text: string }[];
  result: string | null;
  touches: string[];
  makeDefaultLabel: string;
  makeDefault: boolean;
  testLabel: string;
  canTest: boolean;
  testHelp: string;
  connectLabel: string;
  canConnect: boolean;
  cancelLabel: string;
  error: string | null;
  done: boolean;
  request: CloudConnectRequest | null;
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
}

/** `cloud_status`. */
export interface CloudStatusWire {
  default_on?: string;
  providers: CloudProviderWire[];
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

/** `cloud_connect`: the provider as now connected. */
export type CloudConnectWire = CloudProviderWire & { checks?: CloudCheckWire[] };
