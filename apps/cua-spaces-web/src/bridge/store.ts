// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The bridge's state, outside React: one store per provider. It fetches
 * from the adapter, maps through the app core (`derive.ts`), reacts to the
 * host's events, and runs the core's create/power/delete state machine
 * (`creates.reduce` / `creates.compose`), as the Tauri app's
 * `state/cloud.tsx` does. Hooks read it with `useSyncExternalStore`.
 */

import { cloudsInput, cloudsView } from "./clouds";
import { hostOs } from "./host-os";
import { spokenName } from "./space-names";
import { isUnsupported, type DataAdapter } from "./adapter";
import type { AgentMessageSent, AgentRunStatus, AgentSetupRow, AgentStarted, AgentStartRequest, PersistentAgent, SpaceAgentRun } from "./contracts/agents";
import type { CloudStatusWire } from "./contracts/new-space";
import type { CoreClient } from "./core";
import { withCreateContext } from "./create-context";
import { HOST_RUNS_CREATES, loadCreateStreaks, saveCreateStreaks } from "./create-retry";
import { createReport, reportMailto, type CreateReport, type ReportSend } from "./diagnostic-report";
import type { AboutInput } from "./contracts/settings";
import { passedSteps } from "./onboarding";
import type { MachineRow, MachinesView, OnboardingMode, SettingsPage, SignInFailure, SignInPhase, SignInStart, SpaceCreateConfig, TelemetryView } from "./contracts/host";
import type { OnboardingAction, OnboardingFlowState } from "./contracts/onboarding";
import type { KeyvaultOverview, KvGrant, KvListView, KvSelection, VaultAction, VaultState } from "./contracts/keyvault";
import type { KvAccessCommand } from "./ops/keyvault-manage";
import type { BulkResult } from "./contracts/spaces-page";
import { LIST_COMPLETE, type CreateAction, type CreatingElsewhere, type CreatesState, type ListStatus, type Space, type SpaceOs, type SpaceRow } from "./contracts/spaces";
import {
  INITIAL_VAULT_STATE,
  agentRows,
  harnessName,
  orderRuns,
  NO_CREATES,
  composeCreates,
  keyvaultPane,
  groupByApp,
  hostPanelFromState,
  hostState,
  machinesView,
  keyvaultViews,
  reduceCreates,
  rowsToSpaces,
  settingsPage,
  sharesDesktop,
  toMachines,
  validateImageRef,
  type KeyvaultAppGroup,
  type KeyvaultViews,
  type Machine,
} from "./derive";
import type { HostEvent, HostSignIn, HostSignInFailure, SessionSnapshot, SettingKey, SettingsSnapshot, SettingsValues } from "./protocol";
import { signInFailedFromError, signInFailedPhase, type FailedPhase } from "./sign-in";
import { createSettingsFeature, type SettingsFeature } from "./settings-feature";
import { SpaceDetailStore, THIS_MACHINE_ID } from "./space-detail";
import {
  HOST_RECORDS_LAUNCH,
  TelemetryForwarder,
  telemetryCreates,
  telemetryLaunched,
  telemetryOnboarding,
  telemetryOnboardingFinished,
  telemetryOnboardingSkipped,
  telemetrySignInFailure,
} from "./telemetry";
import { ThisMachineStore } from "./this-machine";
import { Transcript, type TranscriptItem } from "./transcript";

export type ResourceName = "spaces" | "machines" | "settings" | "keyvault" | "session" | "agents";

export interface Resource<T> {
  data: T | undefined;
  isLoading: boolean;
  error: Error | null;
}

export interface SettingsData extends SettingsSnapshot {
  /** The Settings page as the app core lays it out; null without the core. */
  page: SettingsPage | null;
}

export interface KeyvaultData {
  overview: KeyvaultOverview;
  /** Items grouped by the app that holds them (plain; works without the core). */
  groups: KeyvaultAppGroup[];
  /** The app core's page, sidebar and vault list; null without the core. */
  views: KeyvaultViews | null;
  /** The vault list's own state (query, selection, open groups). */
  vaultState: VaultState;
}

export interface SessionData extends SessionSnapshot {
  signedIn: boolean;
  identity: string | null;
  /** Where a sign-in started from this UI stands. */
  signIn: SignInPhase;
  /** The page a waiting sign-in finishes on (to open it again). */
  signInUrl: string | null;
}


/** What a persistent agent is doing: paused, a turn running, or waiting. */
export type AgentState = "running" | "paused" | "idle";

/** A run, by where it is. */
export interface RunRef {
  spaceId: string;
  runId: string;
}

export interface AgentSummary {
  name: string;
  harness: string;
  harnessName: string;
  /** The Space it works in, and that Space's name when the Space is known. */
  spaceId: string;
  spaceName: string;
  /** `running`, `suspended` or `released`. */
  spaceState: string;
  state: AgentState;
  /** The core's line, `Claude Code in local:design-review`. */
  detail: string;
  /** `Pause` or `Resume`. */
  actionLabel: string;
  /** The last home save (Unix ms; 0: never). */
  lastActivityMs: number;
  /** The run the detail opens: its current run, else its latest in its Space. */
  run: RunRef | null;
  lastError: string | null;
  /** The host's record, as the core's Agents page reads it (the detail's Memory, Routines and Access). */
  record: PersistentAgent;
}

export interface AgentRunSummary extends SpaceAgentRun {
  spaceId: string;
  spaceName: string;
  harnessName: string;
  /** `createdAt` in Unix ms. */
  startedMs: number | null;
  /** The persistent agent whose run it is, when known. */
  agentName: string | null;
}

export interface AgentsData {
  /** Persistent agents, by name. Empty when the host has none or cannot list them. */
  agents: AgentSummary[];
  /** Runs in every running Space: failed and idle first, then newest (the core's order). */
  runs: AgentRunSummary[];
  /** Running Spaces whose runs could not be read, and why. */
  unread: { spaceId: string; spaceName: string; message: string }[];
  /** The host lists persistent agents (`agents.list`). */
  canList: boolean;
}

/** A run's conversation as it streams in (`useAgentTimeline`). */
export interface AgentTimeline {
  items: readonly TranscriptItem[];
  status: AgentRunStatus | null;
  phase: string;
  /** Every event written so far has been read. */
  caughtUp: boolean;
  isLoading: boolean;
  error: Error | null;
  /** The host cannot read a run's events. */
  unsupported: boolean;
  /** When the last event was written (Unix ms). */
  lastEventMs: number | null;
}

interface TimelineEntry {
  ref: RunRef;
  transcript: Transcript;
  cursor: number;
  view: AgentTimeline;
  watchers: number;
  timer?: ReturnType<typeof setTimeout>;
  busy: boolean;
}

const timelineKey = (r: RunRef) => `${r.spaceId}\n${r.runId}`;

/** `createSpace`'s request: the SDK's create config, plus the OS when the image doesn't say. */
export interface CreateSpaceRequest extends SpaceCreateConfig {
  os?: SpaceOs;
  /** The host also posts a system notification when it is ready (the
   * first run's extra Space, which keeps going after the window closes). */
  notifyWhenReady?: boolean;
}

export interface StoreOptions {
  now?: () => number;
  /** How often to advance create progress while creates run (ms). */
  tickMs?: number;
  /** How often a watched run's events are read while a turn runs, and while it waits (ms). */
  eventsPollMs?: { live: number; idle: number };
  /** How long a sign-in may wait for the browser before it fails (ms). */
  signInTimeoutMs?: number;
}

/** A device code lives about this long; a sign-in still waiting after it
 * will not finish. A native host ends its own wait at 10 minutes and says
 * why; this safety net comes after it, so one failure is counted once. */
const SIGN_IN_TIMEOUT_MS = 11 * 60_000;

const CREATE_TICK_MS = 1_000;
/** How often a watched run is read while a turn runs, and while it waits. */
const EVENTS_LIVE_MS = 500;
const EVENTS_IDLE_MS = 4_000;
/** Most pages read in one go when a run has a backlog. */
const EVENTS_MAX_PAGES = 20;

const idle = <T>(): Resource<T> => ({ data: undefined, isLoading: false, error: null });
const toError = (e: unknown) => (e instanceof Error ? e : new Error(String(e)));

function osOf(req: CreateSpaceRequest): SpaceOs {
  if (req.os) return req.os;
  const image = (req.image ?? "").toLowerCase();
  if (/mac/.test(image)) return "macos";
  if (/win/.test(image)) return "windows";
  return "linux";
}

/** A new create row's id (`pending:<random>`). */
export const newPendingId = () => `pending:${Math.random().toString(36).slice(2, 10)}`;

export class BridgeStore {
  readonly adapter: DataAdapter;
  readonly core: CoreClient;
  /** Settings beyond General, and Notifications (`settings-feature.ts`). */
  readonly extras: SettingsFeature;
  /** A Space detail's usage, windows and picture-in-picture panels (`space-detail.ts`). */
  readonly details: SpaceDetailStore;
  /** This machine's setup form and buttons (`this-machine.ts`). */
  readonly thisMachine: ThisMachineStore;
  /** Usage events to the host, while the machine's setting allows (`telemetry.ts`). */
  readonly telemetry: TelemetryForwarder;
  private readonly now: () => number;
  private readonly tickMs: number;
  private readonly eventsPollMs: { live: number; idle: number };

  private resources: { [K in ResourceName]: Resource<unknown> } = {
    spaces: idle(),
    machines: idle(),
    settings: idle(),
    keyvault: idle(),
    session: idle(),
    agents: idle(),
  };
  private ensured = new Set<ResourceName>();
  private listeners = new Set<() => void>();
  private registry: Space[] = [];
  /** The host's word on its last registry read (`DataAdapter.listNotice`). */
  private listNoticeText: string | null = null;
  /** How far the host's last registry read got (`DataAdapter.listStatus`):
   * not read yet until the first answer. */
  private listStatusValue: ListStatus = { loaded: false, incomplete: false, reaching: 0, stale: false };
  /** What your other computers are creating (`DataAdapter.creatingElsewhere`). */
  private creatingElsewhereValue: CreatingElsewhere[] = [];
  /** The host's standing notice (`DataAdapter.hostNotice`). */
  private hostNoticeText: string | null = null;
  private creates: CreatesState = { ...NO_CREATES, streaks: loadCreateStreaks() };
  private machineRows: MachineRow[] = [];
  private settingsSnap: SettingsSnapshot | undefined;
  private sessionSnap: SessionSnapshot | undefined;
  private signInPhase: SignInPhase = { kind: "idle" };
  /** Settings, AI agents (`AppModel.agentRows`, `agentsBusy`, `agentsPending`): null until read. */
  private agentRows: AgentSetupRow[] | null = null;
  private agentsBusy = false;
  private agentsPending: string[] = [];
  private signInUrl: string | null = null;
  private signInTimer: ReturnType<typeof setTimeout> | undefined;
  /** The sign-in in progress asked for a code instead of the browser's way back. */
  private signInByCode = false;
  /** Counts `session.signIn` answers: a session read started before the
   * latest one says nothing about it (an earlier attempt's failure). */
  private signInEpoch = 0;
  private readonly signInTimeoutMs: number;
  private overview: KeyvaultOverview | undefined;
  private vaultState: VaultState = INITIAL_VAULT_STATE;
  /** Apps the vault list has shown: a new one opens, so its sites are in view; one the user closed stays closed. */
  private seenApps = new Set<string>();
  /** Parity harness: this machine's host state as the core takes it. */
  private hostOverride: { state: object | null } | undefined;
  /** A Machines page the core drew from a parity input (`showMachinesView`). */
  private viewOverride: MachinesView | null = null;
  /** Parity: the rows were drawn by `showMachines*`; a later listing (the page's health check) must not replace them. */
  private machinesPinned = false;
  /** The clock `showMachines` lists its rows at (parity); else now. */
  private machinesNow: number | undefined;
  /** `cloud_status`, read with the machines (the Clouds group). */
  private cloudsStatus: CloudStatusWire | null = null;
  /** The Machines page as drawn (`machinesView`). */
  private machinesViewSnap: MachinesView | null = null;
  private inflight = new Map<ResourceName, Promise<void>>();
  private timelines = new Map<string, TimelineEntry>();
  private unsubscribe: (() => void) | undefined;
  private pollTimer: ReturnType<typeof setInterval> | undefined;
  private tickTimer: ReturnType<typeof setInterval> | undefined;
  private disposed = false;

  constructor(adapter: DataAdapter, core: CoreClient, options: StoreOptions = {}) {
    this.adapter = adapter;
    this.core = core;
    this.now = options.now ?? Date.now;
    this.tickMs = options.tickMs ?? CREATE_TICK_MS;
    this.signInTimeoutMs = options.signInTimeoutMs ?? SIGN_IN_TIMEOUT_MS;
    this.eventsPollMs = options.eventsPollMs ?? { live: EVENTS_LIVE_MS, idle: EVENTS_IDLE_MS };
    this.unsubscribe = adapter.subscribe((e) => this.onEvent(e));
    this.extras = createSettingsFeature(this, this.now);
    this.details = new SpaceDetailStore(adapter, core);
    this.thisMachine = new ThisMachineStore(adapter, core, () => this.refresh("machines"));
    this.telemetry = new TelemetryForwarder(adapter, () => this.telemetryView());
  }

  /* ---- usage events (telemetry.ts) ---- */

  /** Records `app_launched` where the host doesn't (BridgeProvider, once per page). */
  launched(): void {
    if (HOST_RECORDS_LAUNCH[this.adapter.mode]) return;
    // With whether the first run is still to finish (the session says).
    void this.launchEligible().then((eligible) => this.telemetry.track(telemetryLaunched(this.core, eligible)));
  }

  private async launchEligible(): Promise<boolean | null> {
    try {
      if (!this.sessionSnap) {
        this.ensure("session");
        await this.inflight.get("session");
      }
      const done = this.sessionSnap?.onboarding?.completed;
      return typeof done === "boolean" ? !done : null;
    } catch {
      return null;
    }
  }

  /** The events a first-run step means, from the state before it. */
  trackOnboarding(before: OnboardingFlowState, action: OnboardingAction): void {
    // The pages this host passes through unseen (Menu bar off a Mac) are not reported.
    const unseen: readonly string[] = passedSteps();
    const seen = telemetryOnboarding(this.core, before, action).filter(
      (s) => s.type !== "onboarding-page" || !unseen.includes(s.page),
    );
    void this.telemetry.track(seen);
  }

  /** One first-run funnel event (`cua_onboarding_step_*`, `cua_onboarding_skipped`):
   * `step` is one of the app core's funnel steps, `action` `step_viewed`,
   * `step_completed`, `step_failed` or `step_skipped`, `detail` the failure's
   * reason or how it was skipped. Fixed words only; the host checks the
   * switch again and fills the step's index, the version, OS and app version. */
  trackFirstRunStep(step: string, action: string, detail = "none"): void {
    void this.telemetry.track([{ type: "onboarding-page", page: step, action, choice: detail }]);
  }

  /** "Set up later": the first run left for now, on its current page. */
  trackOnboardingSkipped(state: OnboardingFlowState): void {
    void this.telemetry.track(telemetryOnboardingSkipped(this.core, state));
  }

  trackOnboardingFinished(state: OnboardingFlowState): void {
    void this.telemetry.track(telemetryOnboardingFinished(this.core, state));
  }

  /** The machine's usage-data setting now, loading it first if needed. */
  private async telemetryView(): Promise<TelemetryView | null> {
    if (!this.settingsSnap) {
      this.ensure("settings");
      await this.inflight.get("settings");
    }
    return this.settingsSnap?.telemetry ?? null;
  }

  /* ---- subscription (useSyncExternalStore) ---- */

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  get<T>(name: ResourceName): Resource<T> {
    return this.resources[name] as Resource<T>;
  }

  /** The Machines page as every UI draws it (null: no core). */
  get machinesView(): MachinesView | null {
    return this.machinesViewSnap;
  }

  /** Renames a machine (`machines.rename`), then lists the machines again. */
  async renameMachine(id: string, name: string): Promise<void> {
    await this.adapter.call("machines.rename", { id, name });
    await this.refresh("machines");
  }

  /** Asks each machine that can be checked for its health
   * (`machines.checkHealth`), then lists the machines again, with their
   * issues. */
  async checkMachinesHealth(): Promise<void> {
    await this.adapter.call("machines.checkHealth", {});
    await this.refresh("machines");
  }

  /** Removes a machine from the account (`machines.remove`), then lists
   * the Spaces and machines again. */
  async removeMachine(machine: string | null, device: string | null): Promise<void> {
    await this.adapter.call("machines.remove", { machine, device });
    await this.refresh("spaces");
    await this.refresh("machines");
  }

  /** Why the listed Spaces may be out of date (null: the host's last read worked). */
  /** How far the list got: an incomplete one (computers still answering) is
   * never "no Spaces". */
  get listStatus(): ListStatus {
    return this.listStatusValue;
  }

  /** The Spaces your other computers are creating now. */
  get creatingElsewhere(): CreatingElsewhere[] {
    return this.creatingElsewhereValue;
  }

  get listNotice(): string | null {
    return this.listNoticeText;
  }

  /** The host's standing notice (null: none). */
  get hostNotice(): string | null {
    return this.hostNoticeText;
  }

  private set(name: ResourceName, next: Partial<Resource<unknown>>): void {
    this.resources = { ...this.resources, [name]: { ...this.resources[name], ...next } };
    this.notify();
  }

  private notify(): void {
    for (const l of [...this.listeners]) l();
  }

  /** Loads `name` the first time a hook asks for it. */
  ensure(name: ResourceName): void {
    if (this.ensured.has(name) || this.disposed) return;
    this.ensured.add(name);
    if (name === "machines") this.ensure("spaces");
    if (name === "settings") this.ensure("session");
    if (name === "agents") this.ensure("spaces");
    if (name === "spaces" && this.adapter.pollSpacesMs) {
      this.pollTimer = setInterval(() => void this.refresh("spaces"), this.adapter.pollSpacesMs);
    }
    void this.refresh(name);
  }

  /** Refetches `name` from the host (deduplicated while one is in flight). */
  refresh(name: ResourceName): Promise<void> {
    const running = this.inflight.get(name);
    if (running) return running;
    const p = this.load(name).finally(() => this.inflight.delete(name));
    this.inflight.set(name, p);
    return p;
  }

  private async load(name: ResourceName): Promise<void> {
    this.set(name, { isLoading: this.resources[name].data === undefined, error: null });
    try {
      switch (name) {
        case "spaces": {
          const rows = await this.adapter.call("spaces.list", {});
          this.listNoticeText = this.adapter.listNotice?.() ?? null;
          this.listStatusValue = this.adapter.listStatus?.() ?? LIST_COMPLETE;
          this.creatingElsewhereValue = this.adapter.creatingElsewhere?.() ?? [];
          this.hostNoticeText = this.adapter.hostNotice?.() ?? null;
          this.applyRows(rows);
          break;
        }
        case "machines": {
          if (!this.machinesPinned) {
            const rows = await this.adapter.call("machines.list", {});
            // Rows shown meanwhile (`showMachines`, parity) must not lose to a listing that was already in flight.
            if (!this.machinesPinned) this.machineRows = rows;
          }
          this.publishMachines();
          // The Clouds group is read beside the machines, without holding them up; a host with no clouds, or one that fails, shows none.
          void this.adapter
            .call("clouds.status", {})
            .catch(() => null)
            .then((status) => {
              if (JSON.stringify(status) === JSON.stringify(this.cloudsStatus)) return;
              this.cloudsStatus = status;
              if (this.resources.machines.data !== undefined) this.publishMachines();
            });
          break;
        }
        case "settings": {
          this.settingsSnap = await this.adapter.call("settings.get", {});
          this.publishSettings();
          break;
        }
        case "keyvault": {
          this.overview = await this.adapter.call("keyvault.overview", {});
          this.publishKeyvault();
          break;
        }
        case "session": {
          const epoch = this.signInEpoch;
          this.sessionSnap = await this.adapter.call("session.get", {});
          this.followHostSignIn(this.sessionSnap.hostSignIn, epoch === this.signInEpoch ? this.sessionSnap.hostSignInFailure : null);
          this.publishSession();
          if (this.settingsSnap) this.publishSettings();
          break;
        }
        case "agents": {
          this.set("agents", { data: await this.loadAgents(), isLoading: false, error: null });
          break;
        }
      }
    } catch (e) {
      if (!this.disposed) this.set(name, { isLoading: false, error: toError(e) });
    }
  }

  /* ---- spaces ---- */

  private applyRows(rows: SpaceRow[]): void {
    // A create the host runs keeps its progress and, once failed, its
    // reason (not a nameless "stopped" Space).
    const progress = new Map(rows.filter((r) => r.hostProgress).map((r) => [r.id, r.hostProgress!]));
    this.registry = rowsToSpaces(
      this.core,
      rows.map(({ hostProgress: _, ...r }) => r),
      this.now(),
    ).map((s) => {
      const p = progress.get(s.id);
      return p ? { ...s, progress: p, status: p.error ? "suspended" : "provisioning", detail: p.error ?? s.detail } : s;
    });
    this.creates =
      this.core.tryCall<CreatesState>("creates.settle", { state: this.creates, spaces: this.registry }) ?? this.creates;
    this.publishSpaces();
  }

  /** Each finished create's Space id, by the pending id its row had. */
  private readonly createdIds = new Map<string, string>();
  /** The machine a create on one of your machines runs on, by pending id. */
  private readonly pendingHosts = new Map<string, string>();

  /** The Space a finished create became (its row's pending id): a page
   * open on the creating row follows it once the registry lists it. */
  createdId(pendingId: string): string | undefined {
    return this.createdIds.get(pendingId);
  }

  private publishSpaces(): void {
    const spaces = composeCreates(this.core, this.registry, this.creates).map((s) => {
      const host = this.pendingHosts.get(s.id);
      return host && !s.hostName ? { ...s, hostName: host } : s;
    });
    this.set("spaces", { data: spaces, isLoading: false, error: null });
    if (this.resources.machines.data !== undefined) this.publishMachines();
    this.syncTick();
  }

  private dispatch(action: CreateAction): void {
    // A host that runs the create records its usage events itself, from
    // the same core decisions (`AppModel.sendCreate`): sending the page's
    // copy too counted every create twice.
    if (!HOST_RUNS_CREATES[this.adapter.mode]) void this.telemetry.track(telemetryCreates(this.core, this.creates, action, this.now()));
    const before = this.creates;
    this.creates = reduceCreates(this.core, this.creates, action);
    if (this.creates.streaks !== before.streaks) saveCreateStreaks(this.creates.streaks);
    this.publishSpaces();
    if (action.type === "fail" || action.type === "tick" || action.type === "progress") this.noteFailures(before);
  }

  /** Creates already listed in Notifications as failed (by pending id). */
  private readonly failuresNoted = new Set<string>();

  /** Every create that just failed gets one "Couldn't create" entry,
   * however it failed: the host said no, or it stalled (the 4 min "made no
   * progress", which never ends the host's call). */
  private noteFailures(before: CreatesState): void {
    const failedBefore = new Set(before.pending.filter((p) => p.error).map((p) => p.id));
    for (const p of this.creates.pending) {
      if (!p.error || failedBefore.has(p.id) || this.failuresNoted.has(p.id)) continue;
      this.failuresNoted.add(p.id);
      const shown = (this.resources.spaces.data as Space[] | undefined)?.find((s) => s.id === p.id);
      const name = shown?.name || p.name.trim() || "a Space";
      // Why, in the words the failed row shows.
      this.extras.record({ kind: "error", title: `Couldn't create ${name}`, body: shown?.progress?.error ?? p.error });
    }
  }

  private syncTick(): void {
    const running = this.creates.pending.some((p) => !p.error && !p.spaceId);
    if (running && !this.tickTimer) {
      this.tickTimer = setInterval(() => this.dispatch({ type: "tick", now: this.now() }), this.tickMs);
    } else if (!running && this.tickTimer) {
      clearInterval(this.tickTimer);
      this.tickTimer = undefined;
    }
  }

  /** `pendingId` (`newPendingId()`): the row's id, when the caller follows it (New Space's "Open when ready"). */
  async createSpace(req: CreateSpaceRequest = {}, pendingId?: string): Promise<Space> {
    return this.beginCreate(req, pendingId).done;
  }

  /** What each create asked for (by pending id), for Try again. */
  private readonly createRequests = new Map<string, CreateSpaceRequest>();

  canRetryCreate(pendingId: string): boolean {
    return this.createRequests.has(pendingId);
  }

  /** Try again on a failed create: the same request as a new create (the
   * failed row goes). Returns the new row's pending id. */
  retryCreate(pendingId: string): string {
    const req = this.createRequests.get(pendingId);
    if (!req) throw new Error("This create can't be tried again. Create the Space again.");
    const { pendingId: next, done } = this.beginCreate(req);
    void this.dismissCreate(pendingId).catch(() => {});
    // The new row shows how it goes; its failure is noted like any other.
    done.catch(() => {});
    return next;
  }

  private beginCreate(req: CreateSpaceRequest, id?: string): { pendingId: string; done: Promise<Space> } {
    if (req.image) {
      const problem = validateImageRef(this.core, req.image);
      if (problem) return { pendingId: "", done: Promise.reject(new Error(problem)) };
    }
    const pendingId = id ?? newPendingId();
    this.createRequests.set(pendingId, req);
    return { pendingId, done: this.runCreate(req, pendingId) };
  }

  private async runCreate(req: CreateSpaceRequest, pendingId: string): Promise<Space> {
    const { os: _os, notifyWhenReady, ...config } = req;
    const os = osOf(req);
    const on = config.on ?? this.settingsSnap?.values.defaultLocation ?? "local";
    // A Space on one of your machines or in your own cloud lists as a relay machine.
    const provider = on === "local" ? on : "relay";
    // The machine a create on one of yours runs on: its id lets the core
    // fold that machine's own record of the Space into this row, its name
    // is what the row and a failure call it.
    const host = on.startsWith("host:") ? on.slice(5) : null;
    const machine = host ? (this.resources.machines.data as { id: string; name: string }[] | undefined)?.find((m) => m.id === host) : undefined;
    if (machine) this.pendingHosts.set(pendingId, machine.name);
    this.dispatch({
      type: "start",
      id: pendingId,
      name: config.name ?? "",
      os,
      provider,
      now: this.now(),
      image: config.image ?? null,
      kind: config.kind === "container" || config.kind === "vm" || config.kind === "microvm" ? config.kind : null,
      gpu: Boolean(config.gpu),
      host,
      hostName: machine?.name ?? null,
    });
    const args = notifyWhenReady ? { config, pendingId, os, notifyWhenReady } : { config, pendingId, os };
    try {
      const row = await this.createOnHost(args, pendingId);
      this.createdIds.set(pendingId, row.id);
      this.dispatch({ type: "finish", id: pendingId, spaceId: row.id });
      this.createRequests.delete(pendingId);
      await this.refresh("spaces");
      const space = (this.resources.spaces.data as Space[] | undefined)?.find((s) => s.id === row.id) ?? rowsToSpaces(this.core, [row], this.now())[0]!;
      // Plain words: a fresh computer inside this one (never "VM" or "container").
      const system = space.os === "macos" ? "macOS" : space.os === "windows" ? "Windows" : space.os === "linux" ? "Linux" : null;
      const body = system ? `A fresh ${system} computer inside ${({ macos: "this Mac", windows: "this PC", linux: "this computer" } as const)[hostOs()]}. Your agent can use it now.` : undefined;
      this.extras.record({ kind: "message", title: `${spokenName(space.name)} is ready`, ...(body && !machine ? { body } : {}), space: space.id });
      return space;
    } catch (e) {
      const message = toError(e).message;
      // The words it failed with, for a diagnostic report the person may send.
      this.createFailures.set(pendingId, { error: message, at: this.now() });
      if (message.startsWith("cancelled")) this.dispatch({ type: "cancel-done", id: pendingId });
      // The failed row (already shown by `createOnHost`), and its one
      // notification (`noteFailures`).
      // Where it ran, for the notice that words the failure (`createFailedText`).
      throw withCreateContext(toError(e), { provider, hostName: machine?.name });
    }
  }

  /** `spaces.create` on the host. A failure the core answers by running the
   * create once more (one that usually passes: the row says "Retrying…")
   * runs it again here, unless the host runs creates and retries on its own
   * side (`HOST_RUNS_CREATES`, telling the page with `CreateProgress.retry`).
   * Any other failure is put on the row and thrown. */
  private async createOnHost(args: { config: SpaceCreateConfig; pendingId: string; os: SpaceOs; notifyWhenReady?: boolean }, pendingId: string): Promise<SpaceRow> {
    for (;;) {
      try {
        return await this.adapter.call("spaces.create", args);
      } catch (e) {
        const message = toError(e).message;
        if (message.startsWith("cancelled")) throw e;
        this.dispatch({ type: "fail", id: pendingId, error: message, now: this.now() });
        const again = !HOST_RUNS_CREATES[this.adapter.mode] && this.creates.pending.some((p) => p.id === pendingId && p.retryPending && !p.cancelling);
        if (!again) throw e;
      }
    }
  }

  /** Each failed create's error text and when it failed, by pending id. */
  private readonly createFailures = new Map<string, { error: string; at: number }>();

  /** Machine names a report hides (Bonjour names have no dots to find). */
  private reportNames(): string[] {
    const machines = (this.resources.machines.data as { name?: string }[] | undefined) ?? [];
    return machines.map((m) => m.name ?? "").filter((n) => n.trim().length > 0);
  }

  /** The diagnostic report for a failed create, redacted by the core, for
   * the review sheet. Null when the row is not a failed create. Builds the
   * report only: nothing is sent. */
  async createReport(pendingId: string): Promise<CreateReport | null> {
    const p = this.creates.pending.find((c) => c.id === pendingId);
    if (!p?.error) return null;
    const failure = this.createFailures.get(pendingId);
    const about = await this.adapter.call("about.get", {}).catch(() => null);
    return createReport(this.core, {
      error: failure?.error ?? p.error,
      shown: p.error,
      phase: p.phase,
      os: p.os,
      provider: p.provider,
      kind: p.kind ?? null,
      image: p.image ?? null,
      gpu: Boolean(p.gpu),
      elapsedMs: failure ? Math.max(0, failure.at - p.startedAt) : null,
      about: (about as AboutInput | null) ?? null,
      names: [...this.reportNames(), ...(p.hostName ? [p.hostName] : [])],
    });
  }

  /** Send on the review sheet: opens a prefilled email to Cua support with
   * the parts the person kept (redacted again) in their mail app, where they
   * send it themselves. */
  async sendCreateReport(send: ReportSend): Promise<void> {
    const link = reportMailto(this.core, send, this.reportNames());
    if (!link) throw new Error("The report could not be prepared. Try again.");
    await this.openExternal(link);
  }

  async cancelCreate(pendingId: string): Promise<void> {
    this.dispatch({ type: "cancel-start", id: pendingId });
    try {
      await this.adapter.call("spaces.cancelCreate", { pendingId });
    } catch (e) {
      this.dispatch({ type: "cancel-fail", id: pendingId, error: toError(e).message });
      throw toError(e);
    }
  }

  /** Clears a failed create's row, here and on the host (the SwiftUI app
   * keeps its own row for the create until it is dismissed: its Delete on a
   * pending id does that), so it doesn't come back as a ghost. */
  async dismissCreate(pendingId: string): Promise<void> {
    this.dispatch({ type: "dismiss", id: pendingId });
    this.createFailures.delete(pendingId);
    // A removed row is not followed to a Space it might still become.
    this.createdIds.delete(pendingId);
    await this.adapter.call("spaces.delete", { spaceId: pendingId }).catch(() => {});
    await this.refresh("spaces");
  }

  async setPower(spaceId: string, on: boolean): Promise<void> {
    this.dispatch({ type: "power-start", id: spaceId, on, now: this.now() });
    try {
      await this.adapter.call("spaces.setPower", { spaceId, on });
      // Done first, then read the registry: the row says "Suspending…" until
      // a refresh shows the Space off (or on), and a host that sends no
      // `spaces.changed` for a power change and doesn't poll (Electron: cua
      // keeps the power state in its daemon, not in spaces.json) would never
      // read again, leaving the row stuck.
      this.dispatch({ type: "power-done", id: spaceId });
      // A read already in flight may predate the change: wait it out, then read again.
      await this.inflight.get("spaces");
      await this.refresh("spaces");
    } catch (e) {
      this.dispatch({ type: "power-fail", id: spaceId, error: toError(e).message });
      throw toError(e);
    }
  }

  /** Deletes the Space; `removeOnly` only forgets it here (it keeps
   * running in your cloud). */
  async deleteSpace(spaceId: string, removeOnly = false): Promise<void> {
    this.dispatch({ type: "delete-start", id: spaceId, now: this.now() });
    try {
      await this.adapter.call("spaces.delete", removeOnly ? { spaceId, removeOnly } : { spaceId });
      this.dispatch({ type: "delete-done", id: spaceId });
      await this.refresh("spaces");
    } catch (e) {
      this.dispatch({ type: "delete-fail", id: spaceId });
      throw toError(e);
    }
  }

  /** Deletes several Spaces at once (the host turns running ones off
   * first). Each row says it is being deleted until its own result comes
   * back; one failure never stops the rest. Answers one result per id. */
  async deleteSpaces(ids: string[]): Promise<BulkResult[]> {
    return this.bulk("spaces.deleteMany", ids);
  }

  /** Takes you off Spaces shared with you (a team's leaves your list), the
   * rows marked as going meanwhile. Answers one result per id. */
  async leaveSpaces(ids: string[]): Promise<BulkResult[]> {
    return this.bulk("spaces.leave", ids);
  }

  private async bulk(op: "spaces.deleteMany" | "spaces.leave", ids: string[]): Promise<BulkResult[]> {
    if (ids.length === 0) return [];
    for (const id of ids) this.dispatch({ type: "delete-start", id, now: this.now() });
    let results: BulkResult[];
    try {
      results = (await this.adapter.call(op, { ids })).results;
    } catch (e) {
      const error = toError(e).message;
      results = ids.map((id) => ({ id, ok: false, error }));
    }
    for (const id of ids) {
      const r = results.find((x) => x.id === id);
      this.dispatch(r?.ok ? { type: "delete-done", id } : { type: "delete-fail", id });
    }
    await this.refresh("spaces");
    // Every id answers, even one the host left out.
    return ids.map((id) => results.find((r) => r.id === id) ?? { id, ok: false, error: "No answer from the app." });
  }

  /** Gives the Space a new name (its id stays the same), then reads the
   * list again so every view shows it. A refusal (a Space shared with you,
   * a name that is taken or breaks the rule) throws the host's plain words
   * and changes nothing. */
  async renameSpace(spaceId: string, name: string): Promise<void> {
    try {
      await this.adapter.call("spaces.rename", { spaceId, name });
    } catch (e) {
      throw toError(e);
    }
    // A read already in flight may predate the rename: wait it out, then read again.
    await this.inflight.get("spaces");
    await this.refresh("spaces");
  }

  /* ---- machines ---- */

  private publishMachines(): void {
    const spaces = (this.resources.spaces.data as Space[] | undefined) ?? [];
    let machines: Machine[] = toMachines(this.machineRows, spaces, this.core, this.machinesNow);
    const override = this.hostOverride;
    if (override) {
      machines = machines.map((m) => (m.current ? { ...m, panel: hostPanelFromState(this.core, override.state) } : m));
    }
    const here = this.machineRows.find((m) => m.current && !m.device)?.host;
    const now = this.machinesNow ?? Math.floor(Date.now() / 1000);
    const cloudsIn = this.cloudsStatus ? cloudsInput(this.core, this.cloudsStatus, now) : undefined;
    const view = machinesView(this.core, {
      clouds: cloudsIn,
      // The host's own drawing, unless a parity run drew This machine itself.
      hostView: this.viewOverride ?? (override ? null : (this.adapter.machinesView?.() ?? null)),
      machines,
      spaces,
      host: override ? override.state : here ? hostState(here) : null,
      nowSecs: this.machinesNow,
    });
    // The Clouds group is the core's too: a host that draws its own page without one gets it here.
    const clouds = view && !view.clouds?.group.title && cloudsIn ? cloudsView(this.core, cloudsIn) : undefined;
    this.machinesViewSnap = view && clouds ? { ...view, clouds } : view;
    this.set("machines", { data: machines, isLoading: false, error: null });
  }

  /* ---- settings ---- */

  private publishSettings(): void {
    if (!this.settingsSnap) return;
    const data: SettingsData = {
      ...this.settingsSnap,
      page: settingsPage(this.core, this.settingsSnap, this.sessionSnap, this.signInPhase, {
        agents: this.agentRows,
        agentsBusy: this.agentsBusy,
        agentsPending: this.agentsPending,
      }),
    };
    this.set("settings", { data, isLoading: false, error: null });
  }

  /** Picks an option on one of the host's own rows (`settings.choose`). */
  async chooseSetting(row: string, option: string): Promise<void> {
    this.settingsSnap = await this.adapter.call("settings.choose", { row, option });
    this.publishSettings();
  }

  async updateSetting<K extends SettingKey>(key: K, value: SettingsValues[K]): Promise<void> {
    const before = this.settingsSnap;
    if (before) {
      this.settingsSnap = { ...before, values: { ...before.values, [key]: value } };
      this.publishSettings();
    }
    try {
      this.settingsSnap = await this.adapter.call("settings.set", { key, value });
      this.publishSettings();
    } catch (e) {
      this.settingsSnap = before;
      this.publishSettings();
      throw toError(e);
    }
  }

  /* ---- keyvault ---- */

  private publishKeyvault(): void {
    const overview = this.overview;
    if (!overview) return;
    for (const id of [...new Set(overview.items.map((i) => i.provider_id))].sort()) {
      if (this.seenApps.has(id)) continue;
      this.seenApps.add(id);
      if (!this.vaultState.expanded.includes(id)) {
        this.vaultState =
          this.core.tryCall<VaultState>("keyvault.vaultReduce", { overview, state: this.vaultState, action: { type: "toggle-open", key: id } }) ??
          this.vaultState;
      }
    }
    const data: KeyvaultData = {
      overview,
      groups: groupByApp(overview.items),
      views: keyvaultViews(this.core, overview, this.now(), this.vaultState),
      vaultState: this.vaultState,
    };
    this.set("keyvault", { data, isLoading: false, error: null });
  }

  /** Search, select and open groups in the vault list (`keyvault.vaultReduce`). */
  vaultAction(action: VaultAction): void {
    if (!this.overview) return;
    this.vaultState =
      this.core.tryCall<VaultState>("keyvault.vaultReduce", { overview: this.overview, state: this.vaultState, action }) ??
      this.vaultState;
    this.publishKeyvault();
  }

  /** The pane for a sidebar selection; null without the core or data. */
  keyvaultPane(selection: KvSelection): KvListView | null {
    return this.overview ? keyvaultPane(this.core, this.overview, selection, this.now()) : null;
  }

  private async keyvaultAction<T>(f: () => Promise<T>): Promise<T> {
    try {
      return await f();
    } finally {
      await this.refresh("keyvault");
    }
  }

  unlockKeyvault(passphrase?: string): Promise<void> {
    return this.keyvaultAction(async () => {
      await this.adapter.call("keyvault.unlock", { passphrase: passphrase ?? null });
    });
  }

  /** Creates the Keyvault (Touch ID, or this passphrase); answers the
   * recovery key to show once (null when the host showed it). */
  setUpKeyvault(passphrase?: string): Promise<string | null> {
    return this.keyvaultAction(async () => (await this.adapter.call("keyvault.setup", { passphrase: passphrase ?? null })).recoveryKey);
  }

  /** Allows (`true`) or stops (`false`) items in unattended rules. Turning it on
   * asks for presence natively (Touch ID); identity providers refuse. */
  setUnattended(itemIds: string[], unattended: boolean): Promise<void> {
    return this.keyvaultAction(async () => {
      await this.adapter.call("keyvault.setUnattended", { itemIds, unattended });
    });
  }

  setKeyvaultDisabled(disabled: boolean): Promise<void> {
    return this.keyvaultAction(async () => {
      await this.adapter.call("keyvault.setDisabled", { disabled });
    });
  }

  approve(requestId: string, items: string[] | null): Promise<KvGrant> {
    return this.keyvaultAction(() => this.adapter.call("keyvault.approve", { requestId, items }));
  }

  deny(requestId: string): Promise<void> {
    return this.keyvaultAction(async () => {
      await this.adapter.call("keyvault.deny", { requestId });
    });
  }

  revokeGrant(id: string): Promise<void> {
    return this.keyvaultAction(async () => {
      await this.adapter.call("keyvault.revokeGrant", { id });
    });
  }

  /** Shows the items' names (the host asks for Touch ID). */
  showKeyvaultItems(): Promise<void> {
    return this.keyvaultAction(async () => {
      await this.adapter.call("keyvault.showItems", {});
    });
  }

  /** Deletes items and wipes their live copies in Spaces, after the host's
   * own confirmation (rejects with code `cancelled` if declined). */
  deleteKeyvaultItems(itemIds: string[]): Promise<void> {
    return this.keyvaultAction(async () => {
      await this.adapter.call("keyvault.delete", { itemIds });
    });
  }

  /** An Access row's command (revoke a grant, remove a rule, wipe a copy). */
  runKeyvaultCommand(command: KvAccessCommand): Promise<void> {
    return this.keyvaultAction(async () => {
      await this.adapter.call("keyvault.run", { command });
    });
  }

  /** Hides delivered copies from the notch; they stay live until wiped. */
  dismissKeyvaultAccess(imports: string[]): Promise<void> {
    return this.keyvaultAction(async () => {
      await this.adapter.call("keyvault.dismiss", { imports });
    });
  }

  /* ---- session ---- */

  /** Signed in with an account at the last reading (undefined before the first). */
  private wasSignedIn: boolean | undefined;
  private addingThisComputer: Promise<boolean> | null = null;

  /**
   * Signing in adds this computer to the account: right after a sign-in
   * (signed out at the last reading, signed in now) a computer that isn't
   * set up yet is set up to provide Spaces with its screen kept private
   * (`quickSetUp` with the `spare` profile). Sharing its desktop is a
   * choice on its page (Share this desktop), never a side effect of a
   * sign-in. A computer already set up keeps its settings. Resolves to
   * whether this computer is set up; never throws. Once at a time.
   */
  addThisComputer(identity: string | null): Promise<boolean> {
    if (this.addingThisComputer) return this.addingThisComputer;
    const run = (async () => {
      const status = (await this.adapter.call("host.status", {}).catch(() => null)) as { configured?: boolean } | null;
      if (!status) return false;
      if (status.configured) return true;
      return this.thisMachine.quickSetUp(identity, "spare").catch(() => false);
    })();
    this.addingThisComputer = run.finally(() => {
      this.addingThisComputer = null;
    });
    return this.addingThisComputer;
  }

  private publishSession(): void {
    if (!this.sessionSnap) return;
    const signedInNow = this.sessionSnap.account.authMode === "user" && Boolean(this.sessionSnap.account.identity);
    if (this.wasSignedIn === false && signedInNow) void this.addThisComputer(this.sessionSnap.account.identity ?? null);
    this.wasSignedIn = signedInNow;
    const identity = this.sessionSnap.account.identity ?? null;
    const data: SessionData = {
      ...this.sessionSnap,
      signedIn: this.sessionSnap.account.authMode !== "none" && this.sessionSnap.account.configured,
      identity,
      signIn: this.signInPhase,
      signInUrl: this.signInPhase.kind === "waiting" ? this.signInUrl : null,
    };
    this.set("session", { data, isLoading: false, error: null });
  }

  /** A native host runs the sign-in and says when it waits (with the code
   * and the page) only after `session.signIn` answered: take them in, so
   * the page shows them. */
  private followHostSignIn(waiting: HostSignIn | null | undefined, failure?: HostSignInFailure | null): void {
    if (this.signInPhase.kind !== "starting" && this.signInPhase.kind !== "waiting") return;
    // A native host signed in, whichever attempt (a retry's, or one before
    // it) finished: the wait is over.
    const snap = this.sessionSnap;
    if (snap && snap.hostSignIn !== undefined && snap.account.authMode === "user" && snap.account.identity) {
      this.stopSignInTimer();
      this.signInPhase = { kind: "idle" };
      return;
    }
    // The host's sign-in failed (the browser never confirmed, the network
    // dropped, the keychain refused): say why now, never keep waiting.
    if (failure) {
      this.signInFailed(
        {
          kind: "failed",
          reason: (failure.reason as SignInFailure) || "unknown",
          message: failure.message,
          detail: failure.detail,
        },
        true,
      );
      return;
    }
    if (!waiting) return;
    if (waiting.url) this.signInUrl = waiting.url;
    // The host fell back to a code (no browser, or its way back is blocked).
    if (waiting.userCode) this.signInByCode = true;
    const opened = waiting.opened !== false;
    if (this.signInPhase.kind === "waiting" && (this.signInPhase.userCode ?? null) === waiting.userCode && (this.signInPhase.opened ?? true) === opened) return;
    this.signInPhase = { kind: "waiting", userCode: waiting.userCode, opened };
  }

  private setSignIn(phase: SignInPhase): void {
    this.signInPhase = phase;
    this.publishSession();
    this.publishSettings();
  }

  /** `code`: sign in with a code to enter on any device, for when the
   * browser never gets back to the app. */
  async signIn(opts: { code?: boolean } = {}): Promise<SignInStart> {
    this.stopSignInTimer();
    this.signInByCode = opts.code === true;
    if (opts.code) this.trackFeature("sign_in_code_instead");
    this.setSignIn({ kind: "starting" });
    try {
      const start = await this.adapter.call("session.signIn", opts.code ? { code: true } : {});
      this.signInEpoch += 1;
      // Cancelled while it started: leave it.
      if (this.signInPhase.kind !== "starting") return start;
      this.signInUrl = start.verificationUri || null;
      // The way it actually runs: a code when the host gave one (also its own fallback).
      this.signInByCode = Boolean(start.userCode) || start.method === "device" || opts.code === true;
      this.trackFeature(this.signInByCode ? "sign_in_started_code" : "sign_in_started_browser");
      this.setSignIn({ kind: "waiting", userCode: start.userCode ?? null });
      this.signInTimer = setTimeout(() => {
        if (this.signInPhase.kind === "waiting") this.signInFailed(signInFailedPhase(this.core, "notConfirmed"));
      }, this.signInTimeoutMs);
      return start;
    } catch (e) {
      // Shown in the sidebar's foot with its reason (and a toast there),
      // so callers need not say it again.
      this.signInFailed(signInFailedFromError(this.core, e));
      throw toError(e);
    }
  }

  /** Stops waiting for the browser (the page can start again). */
  cancelSignIn(): void {
    if (this.signInPhase.kind !== "starting" && this.signInPhase.kind !== "waiting") return;
    this.stopSignInTimer();
    void this.telemetry.track(telemetrySignInFailure(this.core, null));
    this.setSignIn({ kind: "idle" });
  }

  /** A sign-in failed: shown with its reason, and counted by its kind
   * (never its words). */
  private signInFailed(phase: FailedPhase, counted = false): void {
    this.stopSignInTimer();
    // The reason's kind, and the way it was tried (browser or code). A host
    // that ran the sign-in has counted the reason itself.
    if (!counted) void this.telemetry.track(telemetrySignInFailure(this.core, phase.reason ?? "unknown"));
    this.trackFeature(this.signInByCode ? "sign_in_failed_code" : "sign_in_failed_browser");
    this.setSignIn(phase);
  }

  private trackFeature(feature: string): void {
    void this.telemetry.track([{ type: "feature", feature }]);
  }

  private stopSignInTimer(): void {
    if (this.signInTimer) clearTimeout(this.signInTimer);
    this.signInTimer = undefined;
  }

  /** Opens the Space's own desktop window in the host. */
  async openSpace(spaceId: string): Promise<void> {
    const space = this.get<Space[]>("spaces").data?.find((s) => s.id === spaceId);
    await this.adapter.call("spaces.open", { spaceId, name: space?.name, os: space?.os });
  }

  async signOut(): Promise<void> {
    await this.adapter.call("session.signOut", {});
    this.setSignIn({ kind: "idle" });
    await this.refresh("session");
  }

  async completeOnboarding(mode: OnboardingMode, launchAtLogin?: boolean): Promise<void> {
    await this.adapter.call("session.completeOnboarding", launchAtLogin === undefined ? { mode } : { mode, launchAtLogin });
    await this.refresh("session");
  }

  async openExternal(url: string): Promise<void> {
    await this.adapter.call("session.openExternal", { url });
  }

  /* ---- parity harness (parity.ts) ---- */

  /** Draws this registry and create state, as if the host and the user had
   * produced them. No create tick runs, so the frame holds still. */
  showSpaces(registry: Space[], creates: CreatesState): void {
    this.ensured.add("spaces");
    this.registry = registry;
    this.creates = creates;
    this.listStatusValue = LIST_COMPLETE;
    this.set("spaces", { data: composeCreates(this.core, registry, creates), isLoading: false, error: null });
  }

  /** Lists these machine and device rows on the Machines page, as a host
   * reported them (parity). */
  showMachines(rows: MachineRow[], nowSecs: number): void {
    this.ensured.add("machines");
    this.machineRows = rows;
    this.machinesNow = nowSecs;
    this.hostOverride = undefined;
    this.viewOverride = null;
    this.machinesPinned = true;
    this.publishMachines();
  }

  /** Draws the Machines page the core draws for `input`
   * (`machines_view::MachinesViewInput`, as a host would send it, its
   * machines' health included): parity and previews. */
  showMachinesView(input: object): void {
    const view = this.core.tryCall<MachinesView>("machines.view", { input });
    if (!view) return;
    this.ensured.add("machines");
    this.machineRows = view.machines.map((m) => ({ id: m.id, name: m.name, via: m.current ? "local" : "relay", online: m.online, os: m.os, current: m.current || undefined, limits: [] }));
    this.hostOverride = undefined;
    this.viewOverride = view;
    this.machinesPinned = true;
    this.publishMachines();
  }

  /** Draws This machine for the core's host state (null: not reported yet). */
  showHost(state: object | null): void {
    this.ensured.add("machines");
    if (!this.machineRows.some((m) => m.current && !m.device)) {
      this.machineRows = [{ id: "this-mac", name: "This Mac", via: "local", online: true, os: "macos", current: true, limits: [] }, ...this.machineRows];
    }
    this.hostOverride = { state };
    this.viewOverride = null;
    this.publishMachines();
  }

  /** Draws these persistent agents, with no runs. */
  showAgents(agents: PersistentAgent[], now: number): void {
    this.ensured.add("agents");
    this.set("agents", { data: { agents: this.agentSummaries(agents, [], now), runs: [], unread: [], canList: true }, isLoading: false, error: null });
  }

  /** Draws this Keyvault overview with a fresh vault list. */
  showKeyvault(overview: KeyvaultOverview): void {
    this.ensured.add("keyvault");
    this.overview = overview;
    this.vaultState = INITIAL_VAULT_STATE;
    this.seenApps.clear();
    this.publishKeyvault();
  }

  /* ---- agents ---- */

  /** Persistent agents, and the runs in every running Space. */
  private async loadAgents(): Promise<AgentsData> {
    const spacesLoading = this.inflight.get("spaces");
    if (spacesLoading) await spacesLoading;
    else if (this.resources.spaces.data === undefined) await this.refresh("spaces");
    const spaces = (this.resources.spaces.data as Space[] | undefined) ?? [];
    // A machine that does not share its desktop (a spare Mac that only
    // provides Spaces) refuses the processes a run list starts: not asked.
    // Nor is this machine's own entry: it is not a Space with runs.
    const live = spaces.filter(
      (s) =>
        !s.id.startsWith("pending:") &&
        s.id !== THIS_MACHINE_ID &&
        (s.status === "running" || s.status === "approval" || s.status === "local") &&
        !s.power?.off &&
        sharesDesktop(this.core, s) !== false,
    );

    let list: PersistentAgent[] | null = null;
    let listError: unknown = null;
    const [listed, ...perSpace] = await Promise.allSettled([
      this.adapter.call("agents.list", {}),
      ...live.map((s) => this.adapter.call("agents.runs", { spaceId: s.id })),
    ]);
    if (listed!.status === "fulfilled") list = listed!.value;
    else listError = listed!.reason;

    const runs: AgentRunSummary[] = [];
    const unread: AgentsData["unread"] = [];
    let runsUnsupported = live.length > 0;
    perSpace.forEach((r, i) => {
      const space = live[i]!;
      if (r.status === "fulfilled") {
        runsUnsupported = false;
        for (const run of r.value) {
          runs.push({
            ...run,
            spaceId: space.id,
            spaceName: space.name,
            harnessName: harnessName(this.core, run.agent),
            startedMs: run.createdAt === null ? null : run.createdAt * 1000,
            agentName: list?.find((a) => a.runId === run.runId)?.name ?? null,
          });
        }
      } else if (!isUnsupported(r.reason)) {
        runsUnsupported = false;
        unread.push({ spaceId: space.id, spaceName: space.name, message: toError(r.reason).message });
      }
    });

    // Neither list answers on this host: say so instead of showing an empty page.
    if (list === null && (listError === null || isUnsupported(listError)) && runsUnsupported) throw toError(listError);
    if (list === null && listError !== null && !isUnsupported(listError)) throw toError(listError);

    const ordered = orderRuns(this.core, runs);
    return { agents: this.agentSummaries(list ?? [], ordered, this.now()), runs: ordered, unread, canList: list !== null };
  }

  /** Persistent agents in the core's order and words, each with the run its detail opens. */
  private agentSummaries(list: PersistentAgent[], ordered: AgentRunSummary[], now: number): AgentSummary[] {
    const spaces = (this.resources.spaces.data as Space[] | undefined) ?? [];
    const nameOf = (id: string) => spaces.find((s) => s.id === id)?.name ?? id;
    const rows = agentRows(this.core, list, now);
    return rows.flatMap((row) => {
      const a = list.find((x) => x.name === row.name);
      if (!a) return [];
      const current = a.runId ? ordered.find((r) => r.runId === a.runId) : undefined;
      const latest =
        current ??
        ordered
          .filter((r) => r.spaceId === a.space && r.agent === a.harness)
          .sort((x, y) => (y.createdAt ?? 0) - (x.createdAt ?? 0))[0];
      // The core says Running whenever the agent has a run; a run that is
      // waiting for a follow-up reads as idle when its status is known.
      const state: AgentState = a.paused ? "paused" : a.runId && (!current || current.status === "running") ? "running" : "idle";
      return [
        {
          name: a.name,
          harness: a.harness,
          harnessName: harnessName(this.core, a.harness),
          spaceId: a.space,
          spaceName: nameOf(a.space),
          spaceState: a.spaceState,
          state,
          detail: row.detail,
          actionLabel: row.actionLabel,
          lastActivityMs: Math.max(a.savedMs, current?.status === "running" ? now : 0),
          run: latest ? { spaceId: latest.spaceId, runId: latest.runId } : null,
          lastError: a.lastError ?? null,
          record: a,
        },
      ];
    });
  }

  private agentAction(f: () => Promise<unknown>): Promise<void> {
    return f().then(
      () => this.refresh("agents"),
      async (e) => {
        await this.refresh("agents");
        throw toError(e);
      },
    );
  }

  pauseAgent(name: string): Promise<void> {
    return this.agentAction(() => this.adapter.call("agents.pause", { name }));
  }

  resumeAgent(name: string): Promise<void> {
    return this.agentAction(() => this.adapter.call("agents.resume", { name }));
  }

  /** Starts a coding agent in a Space (`agents.start`); the runs are read again. */
  async startRun(req: AgentStartRequest): Promise<AgentStarted> {
    let started: AgentStarted | undefined;
    await this.agentAction(async () => {
      started = await this.adapter.call("agents.start", req);
    });
    return started!;
  }

  /** Sends a reply to a run (`agents.message`); its conversation is read again at once. */
  async sendToRun(ref: RunRef, text: string, force = false): Promise<AgentMessageSent> {
    const sent = await this.adapter.call("agents.message", { spaceId: ref.spaceId, runId: ref.runId, text, ...(force ? { force } : {}) });
    void this.refresh("agents");
    return sent;
  }

  agentSetup(): Promise<AgentSetupRow[]> {
    return this.adapter.call("agents.setup", {});
  }

  configureAgents(agents: string[] | null): Promise<AgentSetupRow[]> {
    return this.adapter.call("agents.configure", { agents });
  }

  /* ---- Settings, AI agents (the SwiftUI app's `AppModel` agent rows) ---- */

  private setAgentRows(rows: AgentSetupRow[] | null): void {
    this.agentRows = rows;
    this.publishSettings();
  }

  /** Reads the coding agents on this machine for Settings, AI agents (`reloadAgents`). */
  async loadAgentRows(): Promise<void> {
    this.setAgentRows(await this.adapter.call("agents.setup", {}));
  }

  /** "Configure all detected agents" (`configureAllAgents`): "Configuring…" until it is done. */
  async configureAllAgents(): Promise<void> {
    if (this.agentsBusy) return;
    this.agentsBusy = true;
    this.publishSettings();
    try {
      this.setAgentRows(await this.adapter.call("agents.configure", { agents: null }));
    } finally {
      this.agentsBusy = false;
      this.publishSettings();
    }
  }

  /** A row's Configure or Remove (`press(row: "agent:<id>")`, which the
   * host runs): "working…" on the row until it is done, then the rows again. */
  async pressAgentRow(row: string): Promise<void> {
    const id = row.slice("agent:".length);
    if (!row.startsWith("agent:") || this.agentsPending.includes(id)) return;
    this.agentsPending = [...this.agentsPending, id];
    this.publishSettings();
    try {
      this.settingsSnap = await this.adapter.call("settings.choose", { row, option: "press" });
      this.setAgentRows(await this.adapter.call("agents.setup", {}));
    } finally {
      this.agentsPending = this.agentsPending.filter((x) => x !== id);
      this.publishSettings();
    }
  }

  /* ---- a run's timeline ---- */

  /** The run's conversation so far; undefined until something watches it. */
  timeline(ref: RunRef): AgentTimeline | undefined {
    return this.timelines.get(timelineKey(ref))?.view;
  }

  /** Reads the run's events while something watches it: every
   * `eventsPollMs.live` while a turn runs, `eventsPollMs.idle` otherwise.
   * Returns the unwatch. */
  watchTimeline(ref: RunRef): () => void {
    const key = timelineKey(ref);
    let entry = this.timelines.get(key);
    if (!entry) {
      entry = {
        ref,
        transcript: new Transcript(),
        cursor: 0,
        watchers: 0,
        busy: false,
        view: { items: [], status: null, phase: "", caughtUp: false, isLoading: true, error: null, unsupported: false, lastEventMs: null },
      };
      this.timelines.set(key, entry);
    }
    entry.watchers += 1;
    if (entry.watchers === 1 && !entry.busy) void this.pollTimeline(entry);
    return () => {
      entry.watchers -= 1;
      if (entry.watchers === 0 && entry.timer) {
        clearTimeout(entry.timer);
        entry.timer = undefined;
      }
    };
  }

  private publishTimeline(entry: TimelineEntry, patch: Partial<AgentTimeline>): void {
    entry.view = { ...entry.view, ...patch };
    this.notify();
  }

  private async pollTimeline(entry: TimelineEntry): Promise<void> {
    if (this.disposed || entry.watchers === 0) return;
    entry.busy = true;
    const before = entry.view.status;
    let next = this.eventsPollMs.idle;
    try {
      for (let page = 0; page < EVENTS_MAX_PAGES; page++) {
        const r = await this.adapter.call("agents.events", { ...entry.ref, cursor: entry.cursor });
        if (this.disposed) return;
        entry.cursor = r.cursor;
        const changed = entry.transcript.absorbAll(r.events);
        const last = r.events[r.events.length - 1];
        this.publishTimeline(entry, {
          ...(changed ? { items: entry.transcript.items } : {}),
          status: r.status,
          phase: r.phase,
          caughtUp: r.caught_up,
          isLoading: false,
          error: null,
          lastEventMs: last ? last.ts_ms : entry.view.lastEventMs,
        });
        if (r.status === "running") next = this.eventsPollMs.live;
        if (r.caught_up) break;
      }
    } catch (e) {
      if (this.disposed) return;
      if (isUnsupported(e)) {
        this.publishTimeline(entry, { isLoading: false, unsupported: true, error: null });
        entry.busy = false;
        return;
      }
      this.publishTimeline(entry, { isLoading: false, error: toError(e) });
    }
    entry.busy = false;
    // A turn started or ended: the lists show it too.
    if (before !== null && before !== entry.view.status && this.ensured.has("agents")) void this.refresh("agents");
    if (entry.watchers > 0 && !this.disposed) entry.timer = setTimeout(() => void this.pollTimeline(entry), next);
  }

  /* ---- events ---- */

  private onEvent(e: HostEvent): void {
    if (this.disposed) return;
    const reload = (name: ResourceName) => {
      if (this.ensured.has(name)) void this.refresh(name);
    };
    switch (e.type) {
      case "spaces.changed":
        reload("spaces");
        // A Space started or stopped: its runs come or go.
        reload("agents");
        break;
      case "spaces.createProgress": {
        const p = e.progress;
        // The host runs the create once more after a failure that usually
        // passes: the same failure here shows "Retrying…" (the core decides
        // alike on both sides).
        if (p.retry) {
          this.dispatch({ type: "fail", id: p.pendingId, error: p.retry.error, errorVariant: p.retry.errorVariant ?? "", now: this.now() });
          break;
        }
        this.dispatch({
          type: "progress",
          id: p.pendingId,
          phase: p.phase,
          fraction: p.fraction ?? null,
          now: this.now(),
          bytesDone: p.bytesDone ?? null,
          bytesTotal: p.bytesTotal ?? null,
          bytesPerSecond: p.bytesPerSecond ?? null,
        });
        break;
      }
      case "machines.changed":
        reload("machines");
        break;
      case "settings.changed":
        reload("settings");
        break;
      case "keyvault.changed":
        reload("keyvault");
        break;
      case "startup.changed":
        // The live services are in: what was read from the stand-ins
        // before (nothing) is read again.
        if (e.state.phase === "ready") {
          reload("spaces");
          reload("machines");
          reload("session");
        }
        break;
      case "session.signedIn":
        this.stopSignInTimer();
        this.signInPhase = { kind: "idle" };
        reload("session");
        reload("spaces");
        reload("machines");
        break;
      case "session.signInFailed":
        // A host that only has words (the Tauri app): the core reads them.
        this.signInFailed(signInFailedFromError(this.core, new Error(e.reason)));
        break;
      case "session.signedOut":
        this.signInPhase = { kind: "idle" };
        reload("session");
        break;
      case "session.changed":
        reload("session");
        reload("settings");
        break;
      case "agents.changed":
        reload("agents");
        break;
    }
  }

  dispose(): void {
    this.disposed = true;
    this.extras.dispose();
    this.unsubscribe?.();
    this.details.dispose();
    this.stopSignInTimer();
    if (this.pollTimer) clearInterval(this.pollTimer);
    if (this.tickTimer) clearInterval(this.tickTimer);
    for (const t of this.timelines.values()) if (t.timer) clearTimeout(t.timer);
    this.timelines.clear();
    this.listeners.clear();
  }
}
