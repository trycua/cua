// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * New Space and "Connect a cloud", from the app core: the wizard's state
 * machine (`wizard.initial` / `reduce` / `view` / `createArgs`) and the
 * sheet's (`cloudConnect.*`), the same the SwiftUI and Tauri apps bind.
 * The page keeps the state; every word, rule and the create's arguments
 * are the core's.
 *
 * The wizard needs the wasm core and a host that answers
 * `spaces.createOptions` (`ops/new-space.ts`). Without either, New Space
 * creates a Space with the defaults. A host can ask the page to open it
 * (`requestNewSpace`: the SwiftUI app's New Space menu items).
 */

import type { CoreClient } from "./core";
import { createContextOf } from "./create-context";
import { HOST_COVERAGE } from "./coverage";
import { hostOs, THIS_MACHINE, type HostOs } from "./host-os";
import type {
  CloudConnectAction,
  CloudConnectInput,
  CloudConnectState,
  CloudConnectView,
  CloudStatusWire,
  ConnectedCloud,
  CreatePlan,
  CreateSpaceArgs,
  NewSpaceOptions,
  WizardAction,
  WizardEnv,
  WizardLocation,
  WizardState,
  WizardView,
} from "./contracts/new-space";
import type { SandboxImage } from "./contracts/onboarding";
import type { Machine } from "./derive";
import { NO_EXPERIMENTS } from "./contracts/volume";
import type { ParityCheckpoint } from "./parity";
import { withNotSharing } from "./sharing";
import type { BridgeStore } from "./store";

/* ---- The core's functions ------------------------------------------------- */

type Args = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

export const wizardInitial = (core: CoreClient, env: WizardEnv) => core.call<WizardState>("wizard.initial", { env });
export const wizardReduce = (core: CoreClient, state: WizardState, action: WizardAction, env: WizardEnv) =>
  core.call<WizardState>("wizard.reduce", { state, action, env });
/** The wizard as drawn, in the words of the system the app runs on (`hostOs`). */
export const wizardView = (core: CoreClient, state: WizardState, env: WizardEnv, os: HostOs = hostOs()) =>
  core.call<WizardView>("wizard.view", { state, env, hostOs: os });
export const wizardCreateArgs = (core: CoreClient, plan: CreatePlan) => core.call<CreateSpaceArgs>("wizard.createArgs", { plan });
export const creatingText = (core: CoreClient, plan: CreatePlan) => core.call<string>("wizard.creatingText", { plan });
/** The notice after a create failed with `error`: the cause in plain words. A
 * create that ran on another of your machines names that machine, not this Mac. */
export const createFailedText = (core: CoreClient, error: unknown) => {
  const where = createContextOf(error);
  const raw = error instanceof Error ? error.message : String(error);
  return core.call<string>("wizard.createFailedText", { error: raw, provider: where?.provider ?? null, hostName: where?.hostName ?? null });
};
/** The SDK's handshake errors in words a person can act on. */
export const friendlyAddError = (core: CoreClient, raw: string) => core.tryCall<string>("wizard.friendlyAddError", { raw }) ?? raw;

export const cloudConnectInitial = (core: CoreClient) => core.call<CloudConnectState>("cloudConnect.initial", {});
export const cloudConnectReduce = (core: CoreClient, input: CloudConnectInput, state: CloudConnectState, action: CloudConnectAction) =>
  core.call<CloudConnectState>("cloudConnect.reduce", { input, state, action });
export const cloudConnectView = (core: CoreClient, input: CloudConnectInput, state: CloudConnectState) =>
  core.call<CloudConnectView>("cloudConnect.view", { input, state });
/** The sheet's providers from `cloud_status`. */
export const connectInput = (core: CoreClient, status: CloudStatusWire | null) =>
  core.call<CloudConnectInput>("cloudConnect.inputFromStatus", { status: status ?? {} });
/** The connected clouds from `cloud_status`. */
export const connectedClouds = (core: CoreClient, status: CloudStatusWire | null) =>
  core.call<ConnectedCloud[]>("cloudConnect.cloudsFromStatus", { status: status ?? {} });
/** Whether a `default.on` value names one of the user's clouds. */
export const isCloudWord = (core: CoreClient, on: string) => core.tryCall<boolean>("cloudConnect.isCloudWord", { on }) ?? false;

/** The wizard runs here: the core has it and the host answers its options. */
export function wizardOffered(core: CoreClient, mode: string): boolean {
  if (!core.methods.includes("wizard.view")) return false;
  const row = HOST_COVERAGE["spaces.createOptions"] as { unsupported?: Record<string, string> };
  return !row.unsupported?.[mode];
}

/* ---- The wizard's env ----------------------------------------------------- */

/** Backend names (`cua runtime doctor`) that run containers. */
const CONTAINER_BACKENDS = ["container", "managed", "docker", "runsc"];

export interface EnvInput {
  options: NewSpaceOptions | null;
  clouds: CloudStatusWire | null;
  /** Settings' `defaultLocation`: `local`, `cloud`, `host:<id>` or a cloud word. */
  defaultLocation: string | undefined;
  cloudAvailable: boolean;
  machines: readonly (Pick<Machine, "id" | "name" | "via" | "online" | "os" | "limits" | "current"> & Pick<Partial<Machine>, "device" | "notSharing">)[] | undefined;
}

function location(core: CoreClient, on: string | undefined): WizardLocation {
  if (!on || on === "local") return "local";
  if (on === "cloud") return "cloud";
  if (on.startsWith("host:")) return "host";
  return isCloudWord(core, on) ? "yours" : "local";
}

/** What the wizard knows (`wizard::WizardEnv`), from what the bridge has. */
export function wizardEnv(core: CoreClient, input: EnvInput): WizardEnv {
  // The host's own env (the SwiftUI app's): its probes, machines and clouds.
  if (input.options?.env) return input.options.env;
  const local = input.options?.local ?? null;
  const canContainer = Boolean(local?.available && local.backends.some((b) => CONTAINER_BACKENDS.includes(b)));
  const canMacos = Boolean(local?.available && local.backends.includes("lume") && local.macosImage);
  const cores = globalThis.navigator?.hardwareConcurrency || 8;
  return {
    defaultLocation: location(core, input.defaultLocation),
    cloudAvailable: input.cloudAvailable,
    localAvailable: local ? canContainer || canMacos : true,
    localReason: local?.error ?? null,
    localBackends: local ? [...local.backends] : null,
    maxCpus: input.options?.maxCpus ?? Math.max(2, Math.min(16, cores)),
    hostArch: local?.hostArch ?? null,
    storage: local?.storage ?? null,
    cloudPricing: input.options?.cloudPricing ?? null,
    clouds: input.clouds ? connectedClouds(core, input.clouds) : [],
    // Only machines `on="host:<id>"` resolves: never a device of its own.
    // One that is online but not shared says so, as the SDK's hosts do.
    hosts: (input.machines ?? [])
      .filter((m) => !m.current && !m.device)
      .map((m) => ({
        id: m.id,
        name: m.name,
        via: m.via,
        online: m.online,
        os: m.os,
        limits: (m.notSharing ? withNotSharing(m.limits, m.notSharing) : m.limits).map((l) => ({ ...l })),
      })),
    experiments: input.options?.experiments ?? NO_EXPERIMENTS,
    gpus: input.options?.gpus ?? null,
  };
}

/* ---- First Space: one click from an empty list ----------------------------- */

/** A one-click create on this Mac with the wizard's defaults. */
export interface FirstSpaceOffer {
  os: "linux" | "macos";
  /** The catalog's image reference. */
  image: string;
  /** "Ubuntu 24.04", "macOS Tahoe 26". */
  name: string;
  /** "1.2 GB download", or "Already downloaded"; null when unknown. */
  size: string | null;
  /** "about 2 min" (a cached image is quicker). */
  time: string;
  /** Why it can't be created here, in the core's words (no room, no runtime). */
  blocked: string | null;
  /** The create's arguments: the wizard's plan with its defaults. */
  args: CreateSpaceArgs;
}

interface SizedImage {
  sizes?: { platforms: { arch: string; download: number }[] } | null;
}

/** Bytes the way the wizard shows them (binary units labeled GB). */
export function sizeText(bytes: number): string {
  const gb = bytes / 1024 ** 3;
  if (gb >= 1) return `${gb < 10 ? gb.toFixed(1) : Math.round(gb)} GB`;
  return `${Math.round(bytes / 1024 ** 2)} MB`;
}

/**
 * The Spaces a first-time user can create in one click: Linux first (a
 * small download, ready in a couple of minutes), then macOS with its real
 * download size and time. Each is the wizard's own plan for that image on
 * this Mac (`wizard.view`), so the same rules apply: a Mac without room or
 * a runtime gets the core's reason instead of a create that fails later.
 * Empty without the core, or when this Mac can't run Spaces at all.
 */
export function firstSpaceOffers(core: CoreClient, env: WizardEnv): FirstSpaceOffer[] {
  if (core.status !== "ready" || !core.methods.includes("wizard.view")) return [];
  const local = { ...env, defaultLocation: "local" as const };
  const start = wizardInitial(core, local);
  const linux = wizardView(core, start, local).image.ref;
  // The catalog's full macOS (the slim one is about as big).
  const mac = (core.tryCall<SandboxImage[]>("wizard.pickerImages") ?? []).filter((i) => i.published && i.os === "macos" && i.group === "canonical");
  const macos = (mac.find((i) => i.tier !== "slim") ?? mac[0])?.ref ?? null;
  const pulled = new Set(local.storage?.pulled ?? []);
  const offers: FirstSpaceOffer[] = [];
  for (const [os, ref] of [
    ["linux", linux],
    ["macos", macos],
  ] as const) {
    if (!ref) continue;
    let state = wizardReduce(core, start, { type: "choose-image", ref }, local);
    state = wizardReduce(core, state, { type: "choose-placement", on: "local" }, local);
    const system = wizardView(core, state, local);
    if (system.image.ref !== ref || system.image.os !== os) continue;
    const resources = wizardView(core, { ...state, step: 1 }, local);
    const platforms = (system.image as SizedImage).sizes?.platforms ?? [];
    const platform = platforms.find((p) => p.arch === local.hostArch) ?? platforms[0];
    const cached = local.storage ? pulled.has(ref) : false;
    offers.push({
      os,
      image: ref,
      name: system.image.name,
      size: cached ? "Already downloaded" : platform ? `${sizeText(platform.download)} download` : null,
      time: os === "linux" ? (cached ? "under a minute" : "about 2 min") : cached ? "a few minutes" : "10 to 30 min the first time",
      blocked: (system.canContinue ? null : (system.placementError ?? `${THIS_MACHINE[hostOs()]} can't run it.`)) ?? resources.resourcesError,
      args: wizardCreateArgs(core, system.plan),
    });
  }
  return offers;
}

/* ---- The empty home: no Spaces yet ----------------------------------------- */

/** One system's tile (`window::EmptyTile`). */
export interface EmptyTile {
  os: "linux" | "macos";
  /** "Linux", "macOS". */
  name: string;
  /** "About 1 minute · 3.2–7.1 GB of disk", "About 22 GB download". */
  detail: string;
}

/** The main window with no Spaces yet (`window::EmptyHome`). */
export interface EmptyHome {
  title: string;
  /** The line under the title. */
  detail: string;
  /** Linux, then macOS where this machine runs it. */
  tiles: EmptyTile[];
}

/** The chrome's empty-home words (`window::MainChrome`, trycua/cua#4870). */
interface EmptyChrome {
  emptyTitle: string;
  emptyDetail: string;
  emptyAction: string;
  emptyLinuxDetail: string;
  emptyMacosAction: string;
  emptyMacosDetail: string;
}

/**
 * The empty home in the core's words (`window.chrome`, what the SwiftUI
 * app's empty window draws: Linux's time and disk, macOS's download from
 * the image catalog). macOS Spaces run on Apple silicon Macs only (Lume),
 * so its tile shows on a Mac whose `hostArch` is arm64 or not known yet;
 * Windows and Linux hosts and Intel Macs offer Linux alone. Null without
 * the core.
 */
export function emptyHome(core: CoreClient, hostArch: string | null | undefined, os: HostOs = hostOs()): EmptyHome | null {
  if (core.status !== "ready") return null;
  const c = core.tryCall<EmptyChrome>("window.chrome", {});
  if (!c?.emptyLinuxDetail) return null;
  const tiles: EmptyTile[] = [{ os: "linux", name: c.emptyAction, detail: c.emptyLinuxDetail }];
  const appleSilicon = !hostArch || ["arm64", "aarch64"].includes(hostArch.toLowerCase());
  if (os === "macos" && appleSilicon) tiles.push({ os: "macos", name: c.emptyMacosAction, detail: c.emptyMacosDetail });
  return { title: c.emptyTitle, detail: c.emptyDetail, tiles };
}

/* ---- The session: what is open, and its state ---------------------------- */

export interface NewSpaceSession {
  open: boolean;
  state: WizardState | null;
  /** Parity: the env the replay used, instead of the live one. */
  pinnedEnv: WizardEnv | null;
  connect: {
    open: boolean;
    state: CloudConnectState | null;
    /** Parity: the sheet's input, instead of the host's `cloud_status`. */
    pinnedInput: CloudConnectInput | null;
  };
  options: NewSpaceOptions | null;
  clouds: CloudStatusWire | null;
  /** The host asked for the wizard ("Run on" preset to `on`); the page
   * opens it once the core is ready. */
  requested: { on: string | null } | null;
  /** "Run on" to choose once the host's options are in (a request's `on`). */
  placeOn: string | null;
}

export const CLOSED_NEW_SPACE: NewSpaceSession = {
  open: false,
  state: null,
  pinnedEnv: null,
  connect: { open: false, state: null, pinnedInput: null },
  options: null,
  clouds: null,
  requested: null,
  placeOn: null,
};

let session: NewSpaceSession = CLOSED_NEW_SPACE;
const listeners = new Set<() => void>();

export function updateNewSpaceSession(f: (s: NewSpaceSession) => NewSpaceSession): void {
  session = f(session);
  for (const l of [...listeners]) l();
}

export const subscribeNewSpaceSession = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};
export const readNewSpaceSession = () => session;

/** The host asks for New Space (its own menu items): the page opens its
 * wizard when it can (`useNewSpaceRequests`). */
export function requestNewSpace(on: string | null): void {
  updateNewSpaceSession((s) => ({ ...s, requested: { on } }));
}

/** Tests: forget everything. */
export function resetNewSpaceSession(): void {
  stopWaitingForReady?.();
  stopWaitingForReady = null;
  updateNewSpaceSession(() => CLOSED_NEW_SPACE);
}

/** The host's options are what it knows so far (it is still starting), or
 * not in yet: ask again. */
export const optionsPending = (options: NewSpaceOptions | null) => !options || options.pending === true;

/** Stops waiting for the host to be ready (`reloadWhenReady`). */
let stopWaitingForReady: (() => void) | null = null;

export async function loadNewSpaceHostData(store: BridgeStore): Promise<void> {
  const [options, clouds] = await Promise.all([
    store.adapter.call("spaces.createOptions", {}).catch(() => null),
    store.adapter.call("clouds.status", {}).catch(() => null),
  ]);
  // A starting host's answer never replaces a full one.
  updateNewSpaceSession((s) => ({
    ...s,
    options: options && !(options.pending && s.options && !s.options.pending) ? options : s.options,
    clouds: clouds ?? s.clouds,
  }));
  if (options?.pending) reloadWhenReady(store);
}

/** Asks for the host's options again once it says it is ready (the SwiftUI
 * app's `startup.changed`), whether or not a page that shows them is open. */
function reloadWhenReady(store: BridgeStore): void {
  if (stopWaitingForReady) return;
  stopWaitingForReady = store.adapter.subscribe((e) => {
    if (e.type !== "startup.changed" || e.state.phase !== "ready") return;
    stopWaitingForReady?.();
    stopWaitingForReady = null;
    void loadNewSpaceHostData(store);
  });
}

/* ---- Parity --------------------------------------------------------------- */

export type NewSpaceCheckpoint =
  | { kind: "wizard"; state: WizardState; env: WizardEnv; view: WizardView }
  | { kind: "cloud-connect"; input: CloudConnectInput; state: CloudConnectState; view: CloudConnectView };

/** The bridge's answer to each wizard and sheet method a parity flow calls. */
export function newSpaceParityMethods(core: CoreClient, record: (c: ParityCheckpoint) => void): Record<string, (a: Args) => unknown> {
  return {
    "wizard.initial": (a) => wizardInitial(core, a.env),
    "wizard.reduce": (a) => wizardReduce(core, a.state, a.action, a.env),
    "wizard.view": (a) => {
      // The goldens are the Mac's words.
      const view = wizardView(core, a.state, a.env, a.hostOs ?? "macos");
      record({ kind: "wizard", state: a.state, env: a.env, view });
      return view;
    },
    "wizard.createArgs": (a) => wizardCreateArgs(core, a.plan),
    "wizard.creatingText": (a) => creatingText(core, a.plan),
    "wizard.createFailedText": (a) => createFailedText(core, a.error),
    "cloudConnect.initial": () => cloudConnectInitial(core),
    "cloudConnect.reduce": (a) => cloudConnectReduce(core, a.input, a.state, a.action),
    "cloudConnect.view": (a) => {
      const view = cloudConnectView(core, a.input, a.state);
      record({ kind: "cloud-connect", input: a.input, state: a.state, view });
      return view;
    },
  };
}

export interface NewSpaceParityHandle {
  /** Opens New Space on this state, drawn with this env. */
  showWizard(state: WizardState, env: WizardEnv): void;
  /** Opens "Connect a cloud" on this state, drawn with this input. */
  showCloudConnect(input: CloudConnectInput, state: CloudConnectState): void;
}

export const newSpaceParityHandle: NewSpaceParityHandle = {
  showWizard: (state, env) => updateNewSpaceSession((s) => ({ ...s, open: true, state, pinnedEnv: env, connect: { ...CLOSED_NEW_SPACE.connect } })),
  showCloudConnect: (input, state) => updateNewSpaceSession((s) => ({ ...s, open: false, connect: { open: true, state, pinnedInput: input } })),
};
