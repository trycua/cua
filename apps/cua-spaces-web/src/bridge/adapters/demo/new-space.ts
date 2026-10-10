// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's New Space and "Connect a cloud" answers: this Mac with
 * Docker and Lume, the GPU option Lume offers, Cua Cloud rates, the Your
 * cloud experiment on, and an account where AWS and Google Cloud have a
 * sign-in on this machine and nothing is connected yet. Test passes;
 * Connect connects. All synthetic.
 */

import { HostError } from "../../adapter";
import type { CloudProviderWire, CloudTarget, NewSpaceOptions, WizardEnv } from "../../contracts/new-space";
import type { StartupState } from "../../ops/startup";
import type { SpaceRow } from "../../contracts/spaces";
import { NO_EXPERIMENTS } from "../../contracts/volume";
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
  cloudPricing: { vcpuHourUsd: 0.044625, memoryGibHourUsd: 0.0223125 },
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
}

export const demoNewSpaceState = (macHost: boolean): DemoNewSpaceState => ({ macHost: macHost ? { vmsRunning: 2 } : null });

/** The SwiftUI host's answer (`WebUIBridge.createOptions`) for this demo Mac. */
export function demoMacHostOptions(defaultLocation: string, vmsRunning: number, pending: boolean): NewSpaceOptions {
  const d = DEMO_NEW_SPACE_OPTIONS;
  const hd = { availableBytes: 212 * GB, totalBytes: 494 * GB, name: "Macintosh HD" };
  const env: WizardEnv & { localDetails: Record<string, string> | null; lumeSource: string | null; linuxSource: string | null } = {
    defaultLocation: defaultLocation === "cloud" ? "cloud" : "local",
    cloudAvailable: !pending,
    localAvailable: true,
    localReason: null,
    localBackends: pending ? null : ["lume", "qemu", "container"],
    localDetails: pending ? null : { managed: "not used: this Mac's own container engine runs Linux Spaces" },
    maxCpus: 10,
    hostArch: "arm64",
    lumeSource: pending ? null : "auto",
    linuxSource: pending ? null : "auto",
    storage: pending ? null : { reserveBytes: 5 * GB, lume: hd, qemu: hd, container: hd, pulled: ["ghcr.io/trycua/linux:24.04"] },
    cloudPricing: pending ? null : d.cloudPricing,
    clouds: [],
    hosts: [],
    experiments: d.experiments,
    gpus: pending ? null : d.gpus,
  };
  return {
    local: { available: env.localAvailable, backends: env.localBackends ?? [], error: null, hostArch: "arm64", storage: env.storage },
    gpus: env.gpus ?? null,
    cloudPricing: env.cloudPricing ?? null,
    experiments: d.experiments,
    maxCpus: 10,
    env,
    macosVmsRunning: pending ? null : vmsRunning,
    ...(pending ? { pending: true } : {}),
  };
}

function demoProviders(): CloudProviderWire[] {
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

const REGION_NAME: Record<string, string> = { aws: "region", gcp: "project", modal: "environment" };

type Handlers = { [K in keyof NewSpaceOperations]: (args: NewSpaceOperations[K]["args"]) => Promise<NewSpaceOperations[K]["result"]> | NewSpaceOperations[K]["result"] };

/** The New Space answers on a demo machine other than the Mac: its own
 * architecture, runtimes and disks, and no Lume or macOS image. */
export function demoNewSpaceOptions(platform: DemoPlatform = MAC_DEMO_PLATFORM): NewSpaceOptions {
  const options = structuredClone(DEMO_NEW_SPACE_OPTIONS);
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
  state: { settings: SettingsValues; rows: SpaceRow[]; platform?: DemoPlatform; macHost?: DemoNewSpaceState["macHost"]; startup?: StartupState };
  wait: (ms: number) => Promise<void>;
  emit: (e: HostEvent) => void;
  step: number;
  now: () => number;
}

/** Spread into the demo adapter's handlers. */
export function newSpaceDemoHandlers({ state, wait, emit, step, now }: NewSpaceDemoContext): Handlers {
  const providers = demoProviders();
  const find = (target: CloudTarget) => {
    const p = providers.find((x) => x.name === target.provider);
    if (!p) throw new HostError(`no cloud named ${target.provider}`, "not_found");
    return p;
  };
  const placeOf = (p: CloudProviderWire, target: CloudTarget) => target.region || target.project || target.environment || p.region || p.project || p.environment || "";

  return {
    "spaces.createOptions": () =>
      state.macHost
        ? demoMacHostOptions(state.settings.defaultLocation, state.macHost.vmsRunning, (state.startup?.phase ?? "ready") !== "ready")
        : demoNewSpaceOptions(state.platform),

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
    }),

    "clouds.test": async ({ target }) => {
      const p = find(target);
      await wait(step * 2);
      if (!p.credentials?.found) {
        return { provider: p.name, ok: false, checks: [{ name: "credentials", ok: false, detail: `no ${p.title} sign-in on this machine` }] };
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
      if (!p.credentials?.found) throw new HostError(`no ${p.title} sign-in on this machine`, "failed");
      const where = placeOf(p, target);
      p.connected = true;
      if (target.region) p.region = target.region;
      if (target.project) p.project = target.project;
      if (target.environment) p.environment = target.environment;
      if (target.profile) p.profile = target.profile;
      p.label = where ? `${p.title} · ${where}` : p.title;
      if (makeDefault) {
        state.settings = { ...state.settings, defaultLocation: p.name };
        emit({ type: "settings.changed" });
      }
      return { ...p, default: makeDefault, checks: [{ name: "credentials", ok: true, detail: "account 210987654321" }] };
    },
  };
}
