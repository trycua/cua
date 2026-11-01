// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's New Space and "Connect a cloud" answers: this Mac with
 * Docker and Lume, the GPU option Lume offers, the Your cloud
 * experiment on, and an account where AWS and Google Cloud have a
 * sign-in on this machine and nothing is connected yet. Test passes;
 * Connect connects. All synthetic.
 */

import { DEMO_DISK_FREE } from "./resources";
import { HostError } from "../../adapter";
import type { CloudConnectWire, CloudProviderWire, CloudResourceWire, CloudTarget, NewSpaceOptions, WizardEnv } from "../../contracts/new-space";
import type { StartupState } from "../../ops/startup";
import type { SpaceRow } from "../../contracts/spaces";
import { NO_EXPERIMENTS } from "../../contracts/volume";
import type { CloudApproveOps } from "../../ops/cloud-approve";
import type { NewSpaceOperations } from "../../ops/new-space";
import type { HostEvent, SettingsValues } from "../../protocol";
import { demoMachine, MAC_DEMO_PLATFORM, type DemoPlatform } from "./platform";

const GB = 1024 ** 3;

export const DEMO_NEW_SPACE_OPTIONS: NewSpaceOptions = {
  local: {
    available: true,
    backends: ["docker", "lume"],
    containerImage: "ghcr.io/trycua/linux:24.04",
    macosImage: "ghcr.io/trycua/macos:26",
    error: null,
    hostArch: "arm64",
    storage: {
      reserveBytes: 5 * GB,
      lume: { availableBytes: 212 * GB, totalBytes: 494 * GB, name: "Macintosh HD" },
      qemu: { availableBytes: 212 * GB, totalBytes: 494 * GB, name: "Macintosh HD" },
      container: { availableBytes: 58 * GB, totalBytes: 100 * GB, name: "Docker Desktop" },
      pulled: ["ghcr.io/trycua/linux:24.04"],
    },
  },
  gpus: [
    {
      runtime: "lume",
      id: "paravirtual",
      label: "GPU acceleration",
      experimental: true,
      supported: true,
      reason: null,
      learnMore: "https://cua.ai/docs/lume/guides/gpu-passthrough",
    },
  ],
  experiments: { ...NO_EXPERIMENTS, yourCloud: true },
  maxCpus: 10,
};

/** `?demo=mac-host`: answer `spaces.createOptions` the way the SwiftUI
 * host does (`WebUIBridge.createOptions`): its own wizard env, `local`,
 * `gpus` and `cloudPricing` from it, the macOS VMs Lume runs on this Mac
 * (two, none of them Spaces), and while the launch is still starting only
 * what it knows, with `pending`. */
export interface DemoNewSpaceState {
  macHost: { vmsRunning: number } | null;
  /** The clouds the demo starts with (`?demo=clouds`, `clouds-budget`, `clouds-untrusted`, `clouds-token`, `clouds-approval`). */
  cloudsStart: DemoClouds | null;
}

/** Which clouds the demo account has connected, and what is wrong with one. */
export type DemoClouds = "connected" | "budget" | "untrusted" | "token" | "approval" | "cli" | "limits" | "revoked";

export const demoNewSpaceState = (macHost: boolean, clouds: DemoClouds | null = null): DemoNewSpaceState => ({
  macHost: macHost ? { vmsRunning: 2 } : null,
  cloudsStart: clouds,
});

/** The SwiftUI host's answer (`WebUIBridge.createOptions`) for this demo Mac. */
export function demoMacHostOptions(_defaultLocation: string, vmsRunning: number, pending: boolean, diskFree?: number): NewSpaceOptions {
  const d = DEMO_NEW_SPACE_OPTIONS;
  const hd = { availableBytes: diskFree ?? 212 * GB, totalBytes: 494 * GB, name: "Macintosh HD" };
  const env: WizardEnv & { localDetails: Record<string, string> | null; lumeSource: string | null; linuxSource: string | null } = {
    defaultLocation: "local",
    localAvailable: true,
    localReason: null,
    localBackends: pending ? null : ["lume", "qemu", "container"],
    localDetails: pending ? null : { managed: "not used: this Mac's own container engine runs Linux Spaces" },
    maxCpus: 10,
    hostArch: "arm64",
    lumeSource: pending ? null : "auto",
    linuxSource: pending ? null : "auto",
    storage: pending ? null : { reserveBytes: 5 * GB, lume: hd, qemu: hd, container: hd, pulled: ["ghcr.io/trycua/linux:24.04"] },
    clouds: [],
    hosts: [],
    experiments: d.experiments,
    gpus: pending ? null : d.gpus,
  };
  return {
    local: { available: env.localAvailable, backends: env.localBackends ?? [], error: null, hostArch: "arm64", storage: env.storage },
    gpus: env.gpus ?? null,
    experiments: d.experiments,
    maxCpus: 10,
    env,
    macosVmsRunning: pending ? null : vmsRunning,
    ...(pending ? { pending: true } : {}),
  };
}

/** Simulated: what Cua runs in the demo's AWS and Hetzner accounts. */
function demoResources(): CloudResourceWire[] {
  const aws = (id: string, name: string, by: string, cost: number): CloudResourceWire => ({
    provider: "aws",
    id,
    type: "instance",
    sandbox: `aws:${name}`,
    state: "running",
    started_by: by,
    cost_usd: cost,
  });
  const hz = (id: string, name: string, by: string, cost: number): CloudResourceWire => ({
    provider: "hetzner",
    id,
    type: "instance",
    sandbox: `hetzner:${name}`,
    state: "running",
    started_by: by,
    cost_usd: cost,
  });
  return [
    aws("i-0a1b2c3d4e5f60412", "rl-env-0412", "agent eval-runner", 0.08),
    aws("i-0a1b2c3d4e5f60413", "rl-env-0413", "agent eval-runner", 0.08),
    aws("i-0a1b2c3d4e5f60301", "build-cache", "ada on Mac mini", 0.05),
    hz("hz-4410001", "ci-runner-01", "routine nightly-build", 0.02),
    hz("hz-4410002", "ci-runner-02", "routine nightly-build", 0.02),
  ];
}

/** Simulated: the demo account's connected AWS and Hetzner, with one thing wrong when `variant` says so. */
function applyDemoClouds(providers: CloudProviderWire[], variant: DemoClouds): void {
  const aws = providers.find((p) => p.name === "aws")!;
  const hz = providers.find((p) => p.name === "hetzner")!;
  Object.assign(aws, {
    connected: true,
    credentials: { found: false },
    account: "210987654321",
    region: "us-west-2",
    label: "AWS · us-west-2",
    auth: "oidc",
    status: "connected",
    budget_usd_month: 200,
    max_size: "4x16",
    spent_month: 42.18,
    running: 3,
    approval: "",
  });
  Object.assign(hz, {
    connected: true,
    credentials: { found: true, source: "saved token" },
    region: "fsn1",
    label: "Hetzner · fsn1",
    auth: "sealed",
    status: "connected",
    budget_usd_month: 50,
    max_size: "4x8",
    spent_month: 6.4,
    running: 2,
    approval: "",
    holders: ["This Mac", "Mac mini", "Linux box"],
  });
  if (variant === "budget") aws.spent_month = 200;
  if (variant === "untrusted") aws.status = "failed";
  if (variant === "token") hz.status = "failed";
  // This person may approve what waits, on every cloud (a real host says so per cloud).
  for (const p of [aws, hz]) p.can_approve = true;
  if (variant === "approval") {
    Object.assign(aws, { status: "waiting-for-approval", approval: "connection", approval_agent: "Claude Code on This Mac", spent_month: 0, running: 0 });
  }
  if (variant === "limits") {
    Object.assign(aws, { status: "waiting-for-approval", approval: "limits", approval_agent: "eval-runner", pending_budget_usd_month: 500, pending_max_size: "8x32" });
  }
  if (variant === "cli") {
    Object.assign(aws, { auth: "cli", profile: "default", credentials: { found: true, source: "AWS CLI profile default" }, suggest_auth: "oidc", budget_usd_month: 150, max_size: "8x32", spent_month: 18.4 });
  }
  if (variant === "revoked") {
    Object.assign(aws, {
      status: "failed",
      issue: "AWS was disconnected on another device, and 3 machines this machine made may still run there. Reconnect, then delete them, or delete them in the AWS console.",
    });
  }
}

function demoProviders(clouds: DemoClouds | null = null): CloudProviderWire[] {
  const providers = demoProvidersBase();
  if (clouds) applyDemoClouds(providers, clouds);
  return providers;
}

function demoProvidersBase(): CloudProviderWire[] {
  return [
    {
      name: "aws",
      title: "AWS",
      tier: "vm",
      connected: false,
      credentials: { found: true, source: "~/.aws profile default" },
      profile: "default",
      region: "us-east-1",
      ttl_hours: 8,
      kinds: [
        { image: "linux", kind: "container", supported: true, machine_type: "t4g.medium", usd_per_hour: 0.0368 },
        { image: "linux-slim", kind: "container", supported: true, machine_type: "t4g.small", usd_per_hour: 0.0184 },
        { image: "windows", kind: "vm", supported: false, reason: "Windows on AWS is not offered yet." },
        { image: "macos", kind: "vm", supported: false, reason: "macOS on AWS needs EC2 Mac (24 h minimum); not offered." },
      ],
    },
    {
      name: "gcp",
      title: "Google Cloud",
      tier: "vm",
      connected: false,
      credentials: { found: true, source: "gcloud account ada@example.com" },
      region: "us-central1",
      ttl_hours: 8,
      kinds: [
        { image: "linux", kind: "container", supported: true, machine_type: "e2-medium", usd_per_hour: 0.0335 },
        { image: "macos", kind: "vm", supported: false, reason: "Google Cloud does not run macOS." },
      ],
    },
    {
      name: "hetzner",
      title: "Hetzner",
      tier: "vm",
      connected: false,
      credentials: { found: false },
      region: "fsn1",
      ttl_hours: 8,
      kinds: [
        { image: "linux", kind: "container", supported: true, machine_type: "cx23", usd_per_hour: 0.0114 },
        { image: "windows", kind: "vm", supported: false, reason: "Hetzner Cloud has no nested virtualization; only containers run." },
      ],
    },
    {
      name: "modal",
      title: "Modal",
      tier: "sandbox",
      connected: false,
      credentials: { found: false },
      environment: "main",
      ttl_hours: 1,
      kinds: [{ image: "linux", kind: "container", supported: true, machine_type: "sandbox", usd_per_hour: 0.047 }],
    },
  ];
}

const REGION_NAME: Record<string, string> = { aws: "region", gcp: "project", hetzner: "location", modal: "environment" };

type Handlers = { [K in keyof (NewSpaceOperations & CloudApproveOps)]: (args: (NewSpaceOperations & CloudApproveOps)[K]["args"]) => Promise<(NewSpaceOperations & CloudApproveOps)[K]["result"]> | (NewSpaceOperations & CloudApproveOps)[K]["result"] };

/** The New Space answers on a demo machine other than the Mac: its own
 * architecture, runtimes and disks, and no Lume or macOS image. */
export function demoNewSpaceOptions(platform: DemoPlatform = MAC_DEMO_PLATFORM, diskFree?: number): NewSpaceOptions {
  const options = structuredClone(DEMO_NEW_SPACE_OPTIONS);
  if (options.local && diskFree !== undefined) {
    // `?demo=low-disk`: the startup disk is what is almost full.
    const s = options.local.storage!;
    options.local.storage = { ...s, lume: { ...s.lume!, availableBytes: diskFree }, qemu: { ...s.qemu!, availableBytes: diskFree } };
  }
  if (options.local && platform.os === "macos") options.local.hostArch = platform.arch;
  if (platform.os === "macos" || !options.local) return options;
  const m = demoMachine(platform);
  const disk = { ...options.local.storage!.lume!, name: m.disk };
  options.local = {
    ...options.local,
    backends: m.backends,
    macosImage: null,
    hostArch: platform.arch,
    storage: { ...options.local.storage!, lume: null, qemu: disk, container: { ...options.local.storage!.container!, name: m.containerDisk } },
  };
  options.gpus = [];
  return options;
}

export interface NewSpaceDemoContext {
  state: { settings: SettingsValues; rows: SpaceRow[]; platform?: DemoPlatform; macHost?: DemoNewSpaceState["macHost"]; cloudsStart?: DemoClouds | null; startup?: StartupState; diskFree?: number };
  wait: (ms: number) => Promise<void>;
  emit: (e: HostEvent) => void;
  step: number;
  now: () => number;
}

/** Spread into the demo adapter's handlers. */
export function newSpaceDemoHandlers({ state, wait, emit, step, now }: NewSpaceDemoContext): Handlers {
  const providers = demoProviders(state.cloudsStart ?? null);
  let resources = state.cloudsStart ? demoResources() : [];
  // A variant that cannot run its machines (a lost trust, a used budget, a waiting approval) still shows what ran.
  if (state.cloudsStart === "approval") resources = resources.filter((r) => r.provider !== "aws");
  const find = (target: CloudTarget) => {
    const p = providers.find((x) => x.name === target.provider);
    if (!p) throw new HostError(`no cloud named ${target.provider}`, "not_found");
    return p;
  };
  const placeOf = (p: CloudProviderWire, target: CloudTarget) => target.region || target.project || target.environment || p.region || p.project || p.environment || "";

  return {
    "spaces.createOptions": () => {
      // Only `?demo=low-disk` / `disk-full` change what New Space sees.
      const tight = state.diskFree !== undefined && state.diskFree < DEMO_DISK_FREE.healthy ? state.diskFree : undefined;
      return state.macHost
        ? demoMacHostOptions(state.settings.defaultLocation, state.macHost.vmsRunning, (state.startup?.phase ?? "ready") !== "ready", tight)
        : demoNewSpaceOptions(state.platform, tight);
    },

    "spaces.add": async ({ url, name }) => {
      await wait(step);
      const host = url.replace(/^https?:\/\//, "").split(/[:/]/)[0] ?? "";
      if (!host || !/:\d+/.test(url)) throw new HostError(`connect ${url}: connection refused`, "failed");
      const base = (name?.trim() || host).toLowerCase().replace(/[^a-z0-9-]+/g, "-");
      const row: SpaceRow = {
        id: `direct:${base}`,
        name: base,
        provider: "direct",
        spacesdVersion: "0.6.0",
        features: ["desktop_stream", "window_stream"],
        addedAt: new Date(now()).toISOString(),
        os: "linux",
        reachable: true,
      };
      state.rows = [...state.rows.filter((r) => r.id !== row.id), row];
      emit({ type: "spaces.changed" });
      return row;
    },

    "clouds.status": () => ({
      default_on: state.settings.defaultLocation,
      providers: providers.map((p) => ({ ...p, default: p.name === state.settings.defaultLocation })),
      resources: resources.map((r) => ({ ...r })),
    }),

    "clouds.test": async ({ target }) => {
      const p = find(target);
      await wait(step * 2);
      if (!p.credentials?.found && !target.token) {
        return { provider: p.name, ok: false, checks: [{ name: "credentials", ok: false, detail: p.name === "hetzner" ? "paste a Hetzner project API token" : `no ${p.title} sign-in on this machine` }] };
      }
      const where = placeOf(p, target);
      return {
        provider: p.name,
        ok: true,
        account: "210987654321",
        checks: [
          { name: "credentials", ok: true, detail: "account 210987654321" },
          { name: "permissions", ok: true, detail: `can create in ${where || `the default ${REGION_NAME[p.name] ?? "region"}`} (dry run)` },
        ],
      };
    },

    "clouds.connect": async ({ target, makeDefault }) => {
      const p = find(target);
      await wait(step * 2);
      // AWS through Cua's own sign-in: the page to open first, then the check with the account id.
      if (target.auth === "oidc") {
        const region = target.region || p.region || "us-west-2";
        const limits = { region, budget_usd_month: target.budget_usd_month, max_size: target.max_size };
        if (!target.account_id) {
          return {
            ...p,
            ...limits,
            auth: "oidc",
            status: "needs-trust",
            next: {
              open: `https://console.aws.amazon.com/cloudformation/home?region=${region}#/stacks/quickcreate?stackName=cua-spaces`,
              why: "AWS does not trust Cua yet. Create the cua-spaces stack there, then come back with the account id.",
            },
          } satisfies CloudConnectWire;
        }
        if (!/^\d{12}$/.test(target.account_id)) throw new HostError("the AWS account id is 12 digits", "failed");
        Object.assign(p, limits, { connected: true, auth: "oidc", status: "connected", account: target.account_id, label: `AWS · ${region}`, suggest_auth: "", issue: "", profile: "" });
        return { ...p, checks: [{ name: "credentials", ok: true, detail: `account ${target.account_id}` }], verify: "cua cloud status aws" };
      }
      if (!p.credentials?.found && !target.token) throw new HostError(`no ${p.title} sign-in on this machine`, "failed");
      const where = placeOf(p, target);
      p.connected = true;
      p.status = "connected";
      // The token is used for the checks only; the demo keeps nothing of it.
      if (target.token) p.credentials = { found: true, source: "saved token" };
      if (target.region) p.region = target.region;
      if (target.project) p.project = target.project;
      if (target.environment) p.environment = target.environment;
      if (target.profile) p.profile = target.profile;
      if (target.budget_usd_month !== undefined) p.budget_usd_month = target.budget_usd_month;
      if (target.max_size) p.max_size = target.max_size;
      if (p.name === "hetzner") p.auth = "sealed";
      else if (p.name !== "aws") p.auth = "cli";
      p.label = where ? `${p.title} · ${where}` : p.title;
      if (makeDefault) {
        state.settings = { ...state.settings, defaultLocation: p.name };
        emit({ type: "settings.changed" });
      }
      return { ...p, default: makeDefault, checks: [{ name: "credentials", ok: true, detail: "account 210987654321" }] };
    },

    "clouds.disconnect": async ({ provider, deleteResources }) => {
      const p = find({ provider });
      await wait(step * 2);
      const mine = resources.filter((r) => r.provider === provider);
      if (mine.length && !deleteResources) throw new HostError(`${mine.length} machines Cua created are still running in ${p.title}`, "failed");
      resources = resources.filter((r) => r.provider !== provider);
      Object.assign(p, { connected: false, status: "not-connected", auth: "", approval: "", approval_agent: "", running: 0, spent_month: 0, budget_usd_month: undefined, max_size: "", label: "" });
      return { provider, disconnected: true };
    },

    "clouds.approve": async ({ provider, approve }) => {
      const p = find({ provider });
      await wait(step);
      if (approve && p.approval === "limits") {
        Object.assign(p, {
          status: "connected",
          approval: "",
          approval_agent: "",
          budget_usd_month: p.pending_budget_usd_month ?? p.budget_usd_month,
          max_size: p.pending_max_size || p.max_size,
          pending_budget_usd_month: null,
          pending_max_size: "",
        });
      } else if (approve) Object.assign(p, { status: "connected", approval: "", approval_agent: "" });
      else if (p.approval === "limits") Object.assign(p, { status: "connected", approval: "", approval_agent: "", pending_budget_usd_month: null, pending_max_size: "" });
      else Object.assign(p, { connected: false, status: "not-connected", auth: "", approval: "", approval_agent: "" });
      return null;
    },
  };
}
