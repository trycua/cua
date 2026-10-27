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
  CloudSheetAction,
  CloudSheetInput,
  CloudSheetState,
  CloudSheetView,
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
import type { Machine } from "./derive";
import { NO_EXPERIMENTS } from "./contracts/volume";
import type { ParityCheckpoint } from "./parity";
import { withNotSharing } from "./sharing";
import type { BridgeStore } from "./store";

/* ---- The core's functions ------------------------------------------------- */

type Args = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

/** A seed for the name an unnamed Space gets (40 random bits): the wasm core
 * has no randomness of its own. The core picks the name from it when the
 * wizard opens, shows it, and creates the Space with it. */
export function nameSeed(): number {
  const [high = 0, low = 0] = globalThis.crypto.getRandomValues(new Uint32Array(2));
  return (high & 0xff) * 2 ** 32 + low;
}
export const wizardInitial = (core: CoreClient, env: WizardEnv) =>
  core.call<WizardState>("wizard.initial", { env: env.nameSeed == null ? { ...env, nameSeed: nameSeed() } : env });
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

/** "Connect a cloud": the picker and one sheet per cloud (`cloudSheet.*`). */
export const cloudSheetInitial = (core: CoreClient, input: CloudSheetInput, cloud: string | null = null, limits = false) =>
  core.call<CloudSheetState>("cloudSheet.initial", { input, cloud, limits });
export const cloudSheetReduce = (core: CoreClient, input: CloudSheetInput, state: CloudSheetState, action: CloudSheetAction) =>
  core.call<CloudSheetState>("cloudSheet.reduce", { input, state, action });
export const cloudSheetView = (core: CoreClient, input: CloudSheetInput, state: CloudSheetState) =>
  core.call<CloudSheetView>("cloudSheet.view", { input, state });
/** The sheet's providers from `cloud_status`. */
export const cloudSheetInput = (core: CoreClient, status: CloudStatusWire | null) =>
  core.call<CloudSheetInput>("cloudSheet.inputFromStatus", { status: status ?? {} });
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
  /** Settings' `defaultLocation`: `local`, `host:<id>` or a cloud word. */
  defaultLocation: string | undefined;
  machines: readonly (Pick<Machine, "id" | "name" | "via" | "online" | "os" | "limits" | "current"> & Pick<Partial<Machine>, "device" | "notSharing">)[] | undefined;
}

function location(core: CoreClient, on: string | undefined): WizardLocation {
  if (!on || on === "local") return "local";
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
    localAvailable: local ? canContainer || canMacos : true,
    localReason: local?.error ?? null,
    localBackends: local ? [...local.backends] : null,
    maxCpus: input.options?.maxCpus ?? Math.max(2, Math.min(16, cores)),
    hostArch: local?.hostArch ?? null,
    storage: local?.storage ?? null,
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

/** A one-click create on this machine with the wizard's defaults
 * (`wizard::FirstSpaceOffer`). */
export interface FirstSpaceOffer {
  /** What the person picks: `linux`, `omarchy`, `macos`. */
  id: "linux" | "omarchy" | "macos";
  /** The guest system (its icon): `linux` for Omarchy. */
  os: "linux" | "macos";
  /** The catalog's image reference. */
  image: string;
  /** "Ubuntu 24.04", "Omarchy", "macOS Tahoe 26". */
  name: string;
  /** "1.2 GB download", or "Already downloaded"; null when unknown. */
  size: string | null;
  /** "about 2 min" (a cached image is quicker). */
  time: string;
  /** Why it can't be created here, in the core's words. Only Linux is ever
   * offered blocked; Omarchy and macOS are left out instead. */
  blocked: string | null;
  /** The create's arguments: the wizard's plan with its defaults. */
  args: CreateSpaceArgs;
}

/** Bytes the way the wizard shows them (binary units labeled GB). */
export function sizeText(bytes: number): string {
  const gb = bytes / 1024 ** 3;
  if (gb >= 1) return `${gb < 10 ? gb.toFixed(1) : Math.round(gb)} GB`;
  return `${Math.round(bytes / 1024 ** 2)} MB`;
}

/**
 * The Spaces a first-time user can create in one click, the core's
 * (`wizard.firstSpaceOffers`): Linux first, then Omarchy on an x64 machine
 * with a VM runtime, then macOS on a Mac with Apple silicon. Omarchy and
 * macOS are offered only where they run now and fit (the catalog's
 * download plus the unpacked image); Linux always, with the core's reason
 * when it can't be made. Empty without the core.
 */
export function firstSpaceOffers(core: CoreClient, env: WizardEnv, os: HostOs = hostOs()): FirstSpaceOffer[] {
  if (core.status !== "ready" || !core.methods.includes("wizard.firstSpaceOffers")) return [];
  return core.call<FirstSpaceOffer[]>("wizard.firstSpaceOffers", { env, hostOs: os });
}

/** The wizard's own plan for `ref` on this machine with its defaults, and
 * why it can't be created here (the core's words: a system this machine
 * can't run, no runtime, no room), or null. Null when the core has no such
 * image. */
function localPlan(core: CoreClient, env: WizardEnv, ref: string) {
  const local = { ...env, defaultLocation: "local" as const };
  let state = wizardReduce(core, wizardInitial(core, local), { type: "choose-image", ref }, local);
  state = wizardReduce(core, state, { type: "choose-placement", on: "local" }, local);
  const system = wizardView(core, state, local);
  if (system.image.ref !== ref) return null;
  const resources = wizardView(core, { ...state, step: 1 }, local);
  const blocked = (system.canContinue ? null : (system.placementError ?? `${THIS_MACHINE[hostOs()]} can't run it.`)) ?? resources.resourcesError ?? null;
  return { system, blocked };
}

/** Why `ref` can't be created on this machine now, by the rule the Spaces
 * page's one-click offers use ({@link firstSpaceOffers}); null when it can,
 * or when the core can't say. */
export function localBlocked(core: CoreClient, env: WizardEnv, ref: string): string | null {
  if (core.status !== "ready" || !core.methods.includes("wizard.view")) return null;
  return localPlan(core, env, ref)?.blocked ?? null;
}

/* ---- The empty home: no Spaces yet ----------------------------------------- */

/** One system's tile (`window::EmptyTile`). */
export interface EmptyTile {
  os: "linux" | "macos";
  /** "Linux", "macOS". */
  name: string;
  /** "About 3 minutes · 3.3–7.3 GB of disk", "About 22 GB download". */
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
  // The chrome's Linux line is for the machine the core runs on; the wasm core
  // cannot tell, so with the host's architecture known the core words it for that.
  const linuxDetail = hostArch ? (core.tryCall<string>("window.emptyLinuxDetail", { hostArch }) ?? c.emptyLinuxDetail) : c.emptyLinuxDetail;
  const tiles: EmptyTile[] = [{ os: "linux", name: c.emptyAction, detail: linuxDetail }];
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
    state: CloudSheetState | null;
    /** Preview and tests: the sheet's input, instead of the host's `cloud_status`. */
    pinnedInput: CloudSheetInput | null;
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

export type NewSpaceCheckpoint = { kind: "wizard"; state: WizardState; env: WizardEnv; view: WizardView };

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
    // The older sheet (CLI sign-ins) the native apps still draw: replayed on the core,
    // not drawn here (the web UI draws `cloudSheet`).
    "cloudConnect.initial": () => core.call("cloudConnect.initial", {}),
    "cloudConnect.reduce": (a) => core.call("cloudConnect.reduce", { input: a.input, state: a.state, action: a.action }),
    "cloudConnect.view": (a) => core.call("cloudConnect.view", { input: a.input, state: a.state }),
  };
}

export interface NewSpaceParityHandle {
  /** Opens New Space on this state, drawn with this env. */
  showWizard(state: WizardState, env: WizardEnv): void;
}

export const newSpaceParityHandle: NewSpaceParityHandle = {
  showWizard: (state, env) => updateNewSpaceSession((s) => ({ ...s, open: true, state, pinnedEnv: env, connect: { ...CLOSED_NEW_SPACE.connect } })),
};
