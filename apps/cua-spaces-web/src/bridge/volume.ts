// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Cua Volume in the web UI: the Volume page (`drive.*`), the first run's
 * Cua Volume page (`onboarding.*` with the drive actions), the two
 * miniatures and the cua-driver card (`onboarding.driver*`,
 * `agents.setupSummary`). Every decision is the app core's, as in
 * `PersistentModel.sendDrive` and `OnboardingModel` (SwiftUI); this module
 * runs the commands the core asks for through `ops/volume.ts` and keeps
 * the results.
 *
 * Without the core, the Volume page falls back to a small stand-in and the
 * miniatures are left out. Parity (`parity-volume.ts`) pins a state here so
 * the screens draw exactly what a replay reached.
 */

import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";

import { isUnsupported } from "./adapter";
import type { CoreClient } from "./core";
import type { OnboardingFlowState } from "./contracts/onboarding";
import {
  NO_EXPERIMENTS,
  type AgentSetupOutcome,
  type AgentSetupSummary,
  type DriveAction,
  type DriveCard,
  type DriveCheckInput,
  type DriveFrame,
  type DriveInput,
  type DriveRequest,
  type DriveScene,
  type DriveState,
  type DriveStorageUpdate,
  type DriveMountInput,
  type DriveSyncInput,
  type DriveView,
  type DriverFrame,
  type DriverScene,
  type Experiments,
  type SpaceVolumeError,
  type StorageAction,
  type StorageRequest,
} from "./contracts/volume";
import { useBridge } from "./hooks";

/* ---- The core's answers ---------------------------------------------------------- */

export function driveInitial(core: CoreClient): DriveState {
  return core.tryCall<DriveState>("drive.initial") ?? { busy: true, error: null, request: { kind: "load" } };
}

export function driveReduce(core: CoreClient, state: DriveState, action: DriveAction): DriveState {
  return core.tryCall<DriveState>("drive.reduce", { state, action }) ?? fallbackReduce(state, action);
}

export function driveView(core: CoreClient, input: DriveInput, state: DriveState): DriveView {
  return core.tryCall<DriveView>("drive.view", { input, state }) ?? fallbackView(input, state);
}

const sceneCache = new WeakMap<CoreClient, { driver?: DriverScene | null; drive?: DriveScene | null }>();
const scenes = (core: CoreClient) => {
  let s = sceneCache.get(core);
  if (!s) sceneCache.set(core, (s = {}));
  return s;
};

/** The cua-driver card's miniature (null without the core). */
export function driverScene(core: CoreClient): DriverScene | null {
  const s = scenes(core);
  if (s.driver === undefined) s.driver = core.tryCall<DriverScene>("onboarding.driverPreview") ?? null;
  return s.driver;
}

export function driverFrame(core: CoreClient, tMs: number): DriverFrame | null {
  return core.tryCall<DriverFrame>("onboarding.driverPreviewFrame", { tMs: Math.max(0, Math.floor(tMs)) }) ?? null;
}

/** The Reduce Motion picture. */
export function driverStill(core: CoreClient): DriverFrame | null {
  return core.tryCall<DriverFrame>("onboarding.driverPreviewStill") ?? null;
}

/** The Cua Volume page's miniature (null without the core). */
export function driveScene(core: CoreClient): DriveScene | null {
  const s = scenes(core);
  if (s.drive === undefined) s.drive = core.tryCall<DriveScene>("onboarding.drivePreview") ?? null;
  return s.drive;
}

export function driveFrame(core: CoreClient, tMs: number): DriveFrame | null {
  return core.tryCall<DriveFrame>("onboarding.drivePreviewFrame", { tMs: Math.max(0, Math.floor(tMs)) }) ?? null;
}

export function driveStill(core: CoreClient): DriveFrame | null {
  return core.tryCall<DriveFrame>("onboarding.drivePreviewStill") ?? null;
}

/** One agent's line after setup ("Codex: failed", what changed, what failed). */
export function setupSummary(core: CoreClient, outcomes: AgentSetupOutcome[], agent: string, name: string): AgentSetupSummary {
  return (
    core.tryCall<AgentSetupSummary>("agents.setupSummary", { outcomes, agent, name }) ?? {
      line: `${name}: ${outcomes.some((o) => o.agents.includes(agent) && o.change === "failed") ? "failed" : "done"}`,
      text: "",
      failed: outcomes.filter((o) => o.agents.includes(agent) && o.change === "failed").map((o) => `${o.item}: ${o.detail}`),
    }
  );
}

/** The card's fixed words (`OnboardingCopy`'s AI agents fields). */
export interface DriverCopy {
  agentsDriver: string;
  agentsDriverImage: string;
  agentsSkills: string;
  agentsMcp: string;
  agentsSetUp: string;
  agentsSettingUp: string;
  agentsLooking: string;
  agentsNone: string;
  agentsDoneTitle: string;
}

const FALLBACK_DRIVER_COPY: DriverCopy = {
  agentsDriver: "cua-driver skill for background computer-use",
  agentsDriverImage: "An agent clicking in a background window while you keep working",
  agentsSkills: "cua skills",
  agentsMcp: "cua MCP server",
  agentsSetUp: "Set up",
  agentsSettingUp: "Setting up…",
  agentsLooking: "Looking for agents…",
  agentsNone: "No agents found.",
  agentsDoneTitle: "Your AI agents are set up",
};

export function driverCopy(core: CoreClient): DriverCopy {
  return { ...FALLBACK_DRIVER_COPY, ...(core.tryCall<Partial<DriverCopy>>("onboarding.copy") ?? {}) };
}

/** What a bucket field's edit asks for (null: nothing). */
export function storageEdit(core: CoreClient, id: string, value: string): StorageAction | null {
  return core.tryCall<StorageAction | null>("storage.edit", { id, value }) ?? null;
}

/** What a row's choice asks for (null: nothing). */
export function storageChoose(core: CoreClient, id: string, option: string): StorageAction | null {
  return core.tryCall<StorageAction | null>("storage.choose", { id, option }) ?? null;
}

/* ---- Stand-ins without the core ----------------------------------------------------- */

function fallbackReduce(s: DriveState, a: DriveAction): DriveState {
  const start = (request: DriveRequest): DriveState => (s.busy ? s : { busy: true, error: null, request });
  switch (a.type) {
    case "open-volume":
      return start(a.mounted ? { kind: "reveal", path: a.mounted } : { kind: "mount-and-reveal" });
    case "approve":
    case "deny":
    case "revoke":
      return start({ kind: a.type, id: a.id });
    case "reveal":
    case "resolve":
      return start({ kind: a.type, path: a.path });
    case "done":
      return { ...s, busy: false, request: null };
    case "failed":
      return { busy: false, request: null, error: a.error };
  }
}

function fallbackView(input: DriveInput, state: DriveState): DriveView {
  const mounted = input.mount?.state === "mounted" ? (input.mount.path ?? null) : null;
  const line = (id: string, text: string, actionLabel: string | null = null, secondaryLabel: string | null = null) => ({
    id,
    text,
    trailing: "",
    actionLabel,
    secondaryLabel,
  });
  return {
    title: "Volume",
    requestsTitle: "Requests",
    requests: (input.requests ?? []).map((r) => line(r.id, `${r.principal} asks to ${r.mode === "rw" ? "read and write" : "read"} ${r.prefix}`, "Approve", "Deny")),
    grantsTitle: "Grants",
    grants: (input.grants ?? []).filter((g) => !g.revoked).map((g) => line(g.id, `${g.principal} can ${g.mode === "rw" ? "read and write" : "read"} ${g.prefix}`, "Revoke")),
    grantsEmpty: "Agents have only their own folders.",
    busy: state.busy,
    error: state.error ?? null,
    request: state.request ?? null,
    requestText: null,
    openLabel: input.mount && input.mount.state !== "unsupported" ? "Open in Finder" : null,
    mountPath: mounted,
    mountLine: input.mount ? (mounted ? `In Finder at ${mounted}` : "Not mounted") : null,
    devicesTitle: "Devices",
    devices: [],
    syncNote: null,
    syncError: false,
    conflictsTitle: "Conflicts",
    conflicts: [],
  };
}

/* ---- Parity pins ---------------------------------------------------------------- */

/** A state a parity replay reached, held on screen (`?parity` only). */
export interface VolumePins {
  volume?: { input: DriveInput; state: DriveState };
  /** The first run's state is the saved one; this stops the host's answers
   * (experiments, mount, storage) from replacing it. */
  onboarding?: boolean;
  /** The driver card: the miniature at `tMs` (or the still), and the
   * summaries after a setup. */
  driver?: { tMs: number | "still"; summaries: AgentSetupSummary[] | null };
}

let pins: VolumePins = {};
const pinListeners = new Set<() => void>();

export function setVolumePins(next: VolumePins): void {
  pins = { ...pins, ...next };
  for (const l of pinListeners) l();
}

export function useVolumePins(): VolumePins {
  return useSyncExternalStore(
    (l) => {
      pinListeners.add(l);
      return () => pinListeners.delete(l);
    },
    () => pins,
    () => pins,
  );
}

/* ---- Hooks ------------------------------------------------------------------------------ */

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Settings, Experiments as the host reports them; all off while unknown
 * or when the host can't say. Refetched when settings change. */
export function useExperimentFlags(): Experiments | null {
  const { data } = useBridge();
  const [value, setValue] = useState<Experiments | null>(null);
  useEffect(() => {
    if (!data) return;
    let live = true;
    const load = () =>
      data.call("experiments.get", {}).then(
        (x) => live && setValue({ ...NO_EXPERIMENTS, ...x }),
        () => live && setValue(NO_EXPERIMENTS),
      );
    void load();
    const off = data.subscribe((e) => {
      if (e.type === "settings.changed") void load();
    });
    return () => {
      live = false;
      off();
    };
  }, [data]);
  return value;
}

export interface VolumeHook {
  /** The page as the core draws it; null until the first read. */
  view: DriveView | null;
  /** The host can't read the volume (no daemon tools here). */
  unsupported: boolean;
  /** Sends a page action; runs the command the core asks for. */
  act(action: DriveAction): void;
  /** The Spaces the daemon reports without a Cua Volume, and why. */
  spaceErrors: SpaceVolumeError[];
}

/** How often the sync status is read while the page shows (the native app's 5 s). */
export const VOLUME_SYNC_POLL_MS = 5_000;

/** The Volume page (`PersistentModel.sendDrive`). */
export function useVolume(now: () => number = Date.now): VolumeHook {
  const { core, data } = useBridge();
  const pinned = useVolumePins().volume;
  const [input, setInput] = useState<DriveInput | null>(null);
  const [state, setState] = useState<DriveState>(() => driveInitial(core));
  const [unsupported, setUnsupported] = useState(false);
  const current = useRef(state);
  current.current = state;

  const load = useCallback(async () => {
    if (!data) return;
    const o = await data.call("volume.overview", {});
    setInput({ requests: o.requests, grants: o.grants, mount: o.mount, sync: o.sync, home: o.home });
  }, [data]);

  const reduce = useCallback(
    (action: DriveAction) => {
      const next = driveReduce(core, current.current, action);
      current.current = next;
      setState(next);
      return next;
    },
    [core],
  );

  // First read (the core's `load`), then the sync status every 5 s.
  useEffect(() => {
    if (!data || pinned) return;
    let live = true;
    load().then(
      () => live && reduce({ type: "done" }),
      (e: unknown) => {
        if (!live) return;
        if (isUnsupported(e)) setUnsupported(true);
        reduce({ type: "failed", error: message(e) });
      },
    );
    const id = setInterval(() => void load().catch(() => {}), VOLUME_SYNC_POLL_MS);
    return () => {
      live = false;
      clearInterval(id);
    };
  }, [data, pinned, load, reduce]);

  const act = (action: DriveAction) => {
    if (!data || pinned) return;
    const before = current.current;
    const next = reduce(action);
    const request = next.request;
    if (before.busy || !request || request === before.request) return;
    const run = async () => {
      switch (request.kind) {
        case "load":
          break;
        case "mount-and-reveal": {
          const status = await data.call("volume.mount", {});
          if (status.state !== "mounted" || !status.path) throw new Error(status.detail ?? "The volume could not be mounted");
          await data.call("volume.reveal", { path: status.path });
          break;
        }
        case "approve":
          await data.call("volume.approve", { id: request.id });
          break;
        case "deny":
          await data.call("volume.deny", { id: request.id });
          break;
        case "revoke":
          await data.call("volume.revoke", { id: request.id });
          break;
        case "reveal":
          await data.call("volume.reveal", { path: request.path });
          break;
        case "resolve":
          await data.call("volume.resolve", { path: request.path });
          break;
      }
      await load();
    };
    run().then(
      () => reduce({ type: "done" }),
      (e: unknown) => reduce({ type: "failed", error: message(e) }),
    );
  };

  if (pinned) return { view: driveView(core, pinned.input, pinned.state), unsupported: false, act, spaceErrors: spaceVolumeErrors(pinned.input.sync, pinned.input.mount) };
  return {
    view: input ? driveView(core, { ...input, nowMs: now() }, state) : null,
    unsupported,
    act,
    spaceErrors: spaceVolumeErrors(input?.sync, input?.mount),
  };
}

/** The Spaces the daemon reports without a Cua Volume (`volume_errors` in
 * the sync and mount status; one row per Space). */
export function spaceVolumeErrors(sync?: DriveSyncInput | null, mount?: DriveMountInput | null): SpaceVolumeError[] {
  const seen = new Map<string, SpaceVolumeError>();
  for (const e of [...(sync?.volume_errors ?? []), ...(mount?.volume_errors ?? [])]) {
    if (e && typeof e.space === "string" && typeof e.error === "string" && !seen.has(e.space)) seen.set(e.space, e);
  }
  return [...seen.values()];
}

/* ---- The first run's Cua Volume page ---------------------------------------------------- */

/** While macOS waits for the extension's approval, or the agent prompt
 * shows, the page reads again this often (the native app's 2 s). */
export const DRIVE_ONBOARDING_POLL_MS = 2_000;

interface OnboardingFlow {
  state: OnboardingFlowState | null;
  /** The page as drawn (its `drive` card on the Cua Volume page). */
  view: { drive?: DriveCard | null } | null;
  send(action: { type: string; [k: string]: unknown }): void;
}

/** The drive card on the first run's state (null off its page). */
export function driveCardOf(view: { drive?: DriveCard | null } | null | undefined): DriveCard | null {
  return view?.drive ?? null;
}

const failedCheck = (detail: string): DriveCheckInput => ({ ok: false, reachable: false, authorized: false, versioning: false, detail, applied: false });

/**
 * What the native `OnboardingModel` does around the Cua Volume page: tells
 * the flow which experiments are on, what the mount can do here and where
 * the files live; runs the storage test, save or adoption and the mount the
 * core asks for; and follows the daemon while it waits for approval or for
 * the bucket an agent connects. Held still while parity pins a state.
 */
export function useOnboardingVolume(flow: OnboardingFlow): void {
  const { data } = useBridge();
  const experiments = useExperimentFlags();
  const pinned = Boolean(useVolumePins().onboarding);
  const state = flow.state;
  // An answer still in flight when parity pins a state is dropped.
  const send = useRef(flow.send);
  send.current = (action) => {
    if (!pins.onboarding) flow.send(action);
  };
  const has = Boolean(state);
  const cuaVolume = experiments?.cuaVolume;

  useEffect(() => {
    if (pinned || !has || cuaVolume === undefined) return;
    send.current({ type: "experiments-loaded", experiments: { ...NO_EXPERIMENTS, cuaVolume } });
  }, [pinned, has, cuaVolume]);

  const check = useCallback(async () => {
    if (!data) return;
    const [overview, storage] = await Promise.all([
      data.call("volume.overview", {}).catch(() => null),
      data.call("volume.storage", {}).catch(() => null),
    ]);
    if (storage) send.current({ type: "drive-storage-loaded", storage, home: overview?.home ?? null });
    send.current({ type: "drive-checked", os: overview?.os ?? "macos", status: overview?.mount ?? null });
  }, [data]);

  // Once the experiment is on: what the mount can do here.
  const checked = Boolean(state?.driveChecked);
  useEffect(() => {
    if (pinned || !has || !cuaVolume || checked) return;
    void check();
  }, [pinned, has, cuaVolume, checked, check]);

  // macOS waits for the extension's approval: ask again until it moves on.
  const card = driveCardOf(flow.view);
  const awaiting = Boolean(card?.settingsUrl);
  useEffect(() => {
    if (pinned || !awaiting) return;
    const id = setInterval(() => void check(), DRIVE_ONBOARDING_POLL_MS);
    return () => clearInterval(id);
  }, [pinned, awaiting, check]);

  // The agent prompt shows: look for the bucket it connects.
  const storageState = state?.storage as { request?: StorageRequest | null; busy?: boolean } | undefined;
  const prompting = Boolean(card?.storageRows.some((r) => r.id === "s3-prompt"));
  useEffect(() => {
    if (pinned || !prompting || !data) return;
    const id = setInterval(() => {
      void Promise.all([data.call("volume.storage", {}).catch(() => null), data.call("volume.overview", {}).catch(() => null)]).then(
        ([storage, overview]) => storage && send.current({ type: "drive-storage-loaded", storage, home: overview?.home ?? null }),
      );
    }, DRIVE_ONBOARDING_POLL_MS);
    return () => clearInterval(id);
  }, [pinned, prompting, data]);

  // The storage form's test, Continue's save, or adopting the agent's bucket.
  const storageRequest = storageState?.request ?? null;
  const storageRunning = useRef(false);
  useEffect(() => {
    if (pinned || !data || !storageRequest || storageRunning.current) return;
    const kind = storageRequest.kind;
    if (kind !== "test" && kind !== "save" && kind !== "adopt") return;
    storageRunning.current = true;
    const update = (storageRequest as { update: DriveStorageUpdate }).update;
    const answer = (check: DriveCheckInput) =>
      kind === "test"
        ? { type: "drive-storage", action: { type: "checked", check } }
        : kind === "adopt"
          ? { type: "drive-storage", action: { type: "adopted", check } }
          : { type: "drive-storage-saved", check };
    void data
      .call("volume.storageSet", { update })
      .then(
        (check) => send.current(answer(check)),
        (e: unknown) =>
          send.current(kind === "test" ? { type: "drive-storage", action: { type: "failed", error: message(e) } } : answer(failedCheck(message(e)))),
      )
      .finally(() => {
        storageRunning.current = false;
      });
  }, [pinned, data, storageRequest]);

  // Continue with the box and the daemon disagreeing: mount or unmount.
  const driveRequest = (state?.driveRequest as "mount" | "unmount" | null | undefined) ?? null;
  const mountRunning = useRef(false);
  useEffect(() => {
    if (pinned || !data || !driveRequest || mountRunning.current) return;
    mountRunning.current = true;
    void data
      .call(driveRequest === "mount" ? "volume.mount" : "volume.unmount", {})
      .then(
        (status) => send.current({ type: "drive-mounted", status }),
        (e: unknown) => send.current({ type: "drive-failed", error: message(e) }),
      )
      .finally(() => {
        mountRunning.current = false;
      });
  }, [pinned, data, driveRequest]);
}

/* ---- The cua-driver card ---------------------------------------------------------------- */

export interface DriverSetup {
  /** Sets up cua-driver for `agents`; one summary per agent, in order. */
  setUp(agents: { id: string; name: string }[]): Promise<AgentSetupSummary[]>;
}

/** `cua agents setup --cua-driver` through `agents.setupDriver`, summarised by the core. */
export function useDriverSetup(): DriverSetup {
  const { core, data } = useBridge();
  return {
    setUp: async (agents) => {
      if (!data) throw new Error("The bridge is still loading");
      const outcomes = await data.call("agents.setupDriver", { agents: agents.map((a) => a.id) });
      return agents.map((a) => setupSummary(core, outcomes, a.id, a.name));
    },
  };
}
