// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The app's root model (the SwiftUI app's AppModel.swift): the core's Space
// list state, the creates, deletes and power changes running, the account
// and the app settings. It forwards every decision to the app core
// (`appRosterReduce`, `appSidebar`, `appSpaceDetail`, `appCreatesReduce`,
// ...); the bridge answers the page from what it exposes. Each change says
// what moved (`subscribe`), so the bridge tells the page.
import { randomUUID } from "node:crypto";
import type { Native } from "../native/load";
import type {
  AppCreateAction,
  AppCreatesState,
  AppCreateSpaceArgs,
  AppGpuChoice,
  AppCloudPricing,
  AppLocalStorage,
  AppMainChrome,
  AppRosterAction,
  AppRosterState,
  AppSettings,
  AppSidebarView,
  AppSignInPhase,
  AppSpace,
  AppSpaceDetail,
  AppSpaceHost,
  AppSpaceOs,
  AppSpaceUsage,
  AppWizardEnv,
  LocalStorage,
  SpaceCreateProgress,
} from "../native/generated/index";
import type { LocalRuntimes, SpacesBackend } from "./backend";
import { AgentsModel, type AgentSetupRunning, type ToolRunner } from "./agents";
import type { CloudModel } from "./cloud";
import type { DevicesModel } from "./devices";
import { NotchActivity } from "./activity";
import { isCancelled, words } from "./errors";
import type { HostModel } from "./host";
import { KeyvaultModel } from "./keyvault";
import type { AccountRunning, TelemetryRunning } from "./services";
import type { StartupModel } from "./startup";
import { SpaceThumbnails } from "./thumbnails";
import { TimeoutError, sleep, withTimeout } from "./time";

/** What changed, for the page's events. */
export type ModelChange = "spaces" | "session" | "settings" | "machines" | "agents" | "keyvault";

export interface AppModelDeps {
  native: Native;
  backend: SpacesBackend;
  startup: StartupModel;
  settingsPath: string;
  host: HostModel;
  devices: DevicesModel;
  cloud: CloudModel;
  account: AccountRunning | null;
  telemetry: TelemetryRunning | null;
  /** The coding agents on this machine (null: none can be set up in this build). */
  agentSetup?: AgentSetupRunning | null;
  /** The daemon's tools for the agents and their keys (default: the backend's; null: no daemon). */
  agentTools?: ToolRunner | null;
  /** The Keyvault (a model without a broker when omitted). */
  keyvault?: KeyvaultModel;
  /** The live services are in (the launch's stand-ins have them). */
  servicesIn: () => boolean;
  /** CPUs this machine has (the wizard's limit). */
  cpus: number;
}

/** The create the daemon never took. */
export class CreateNotAccepted extends Error {
  constructor(seconds: number) {
    super(`Cua's background service didn't start this create within ${seconds} s. Cua Spaces reconnected to it. Try again.`);
    this.name = "CreateNotAccepted";
  }
}

const TICK_MS = 250;
/** How often the refresh checks relay sharing against the sign-in (ms). */
const ACCOUNT_CHECK_MS = 120_000;

export class AppModel {
  readonly native: Native;
  readonly backend: SpacesBackend;
  readonly startup: StartupModel;
  readonly host: HostModel;
  readonly devices: DevicesModel;
  readonly cloud: CloudModel;
  readonly account: AccountRunning | null;
  readonly telemetry: TelemetryRunning | null;
  /** Agents: the persistent agents, a Space's runs, the provider keys and the coding agents. */
  readonly agents: AgentsModel;
  readonly keyvault: KeyvaultModel;
  readonly settingsPath: string;
  settings: AppSettings;
  roster: AppRosterState;
  /** The selected Space (the sidebar's selection). */
  selectedSpaceId: string | null = null;
  loaded = false;
  /** The last list read failed: the page says so above every page, and the
   * rows already listed stay (the SwiftUI app's `rosterError`). Never the
   * error's own words: they may carry a server's body or a credential-bearing URL. */
  rosterError: string | null = null;
  banner: string | null = null;
  bannerIsError = false;
  /** The signed-in account. */
  identity: string | null;
  /** Cua Cloud can be used. */
  cloudConfigured = false;
  signIn: AppSignInPhase;
  /** The page a waiting sign-in finishes on. */
  signInUrl: string | null = null;
  /** A sign-in still waiting after this fails (a device code lives about this long). */
  signInTimeoutMs = 600_000;
  /** The registry's Spaces (before This machine is added). */
  registrySpaces: AppSpace[] = [];
  /** Memory and storage use of the Spaces whose detail is showing. */
  readonly usage = new Map<string, AppSpaceUsage>();
  /** Each Space's latest thumbnail (the notch tiles and the page's previews). */
  readonly thumbnails: SpaceThumbnails;
  /** The notch's indicator: the hotspot, a transfer running (a teleport, files dropped on the notch). */
  readonly activity = new NotchActivity();
  creates: AppCreatesState = { pending: [], deleting: [], powering: [] };
  /** macOS VMs running on this Mac, as New Space last read them. */
  runningMacosVms: number | null = null;
  lumeSource: string | null = null;
  linuxSource: string | null = null;
  /** Your machines that provide Spaces, as New Space last read them. */
  hosts: AppSpaceHost[] = [];
  /** How long one read of the registry may take before a refresh gives up on it (s). */
  listTimeout = 20;
  /** How long the host's own reads in a refresh may take (s). */
  hostTimeout = 15;
  /** With no successful list read for this long, the daemon connection is made again (ms). */
  listStaleAfterMs = 45_000;
  lastListOk: number | null = null;
  /** Makes a new SDK client of the daemon; false when it could not (set once the services are in). */
  reconnect: (() => Promise<boolean>) | null = null;
  reconnecting = false;
  reconnects = 0;
  /** How long the daemon may take to take a create (s). */
  createAcceptTimeout = 30;
  readonly maxCpus: number;
  readonly servicesInNow: () => boolean;

  private signInAttempt = 0;
  private listWatchedSince = Date.now();
  private rowsInFlight: Promise<import("../native/generated/index").AppSpaceRow[]> | null = null;
  private hostInFlight: Promise<void> | null = null;
  private createTicker: NodeJS.Timeout | null = null;
  private readonly listeners = new Set<(change: ModelChange) => void>();

  constructor(d: AppModelDeps) {
    this.native = d.native;
    this.backend = d.backend;
    this.startup = d.startup;
    this.host = d.host;
    this.devices = d.devices;
    this.cloud = d.cloud;
    this.account = d.account;
    this.telemetry = d.telemetry;
    this.agents = new AgentsModel(
      d.native,
      d.agentTools === undefined ? (tool, args) => d.backend.tool(tool, args) : d.agentTools,
      d.agentSetup ?? null,
      (id) => d.backend.agentRuns(id),
    );
    this.agents.onChange(() => this.changed("agents"));
    this.agents.persistent.thisMachine = () => {
      const id = this.sidebar.thisMachine?.id;
      return id?.startsWith("relay:") ? id : null;
    };
    this.keyvault = d.keyvault ?? new KeyvaultModel(d.native, null);
    this.settingsPath = d.settingsPath;
    this.servicesInNow = d.servicesIn;
    this.maxCpus = Math.max(2, Math.min(16, d.cpus));
    this.settings = d.native.appSettingsLoad(d.settingsPath);
    const policy = d.native.appThumbnailPolicy();
    this.thumbnails = new SpaceThumbnails(
      { openIntervalMs: Number(policy.openIntervalMs), backgroundIntervalMs: Number(policy.backgroundIntervalMs), maxDimension: policy.maxDimension },
      (id, maxAgeMs) => this.backend.thumbnail(id, maxAgeMs),
    );
    // Live Keyvault sign-ins show left of the notch (and in the menu).
    // Dismissed ones stay hidden across launches (the settings file).
    this.keyvault.dismissed = this.settings.dismissedAccess;
    this.keyvault.onDismissed = (ids) => {
      this.settings.dismissedAccess = ids;
      this.saveSettings();
    };
    this.keyvault.subscribe(() => this.changed("keyvault"));
    this.roster = d.native.appRosterInitial([]);
    this.signIn = new d.native.AppSignInPhase.Idle();
    this.identity = d.account?.identity() ?? null;
    this.host.identity = this.identity;
    this.devices.signedIn = this.identity !== null;
    // An approval (the minute's read, Check again) reaches an open detail: its access
    // notice goes and its stream opens without reopening it.
    this.devices.onChange = () => this.changed("machines");
    this.host.onChange = () => {
      this.recompose();
      this.changed("machines");
    };
    // "Set up for access" signs in inline when relay setup has no account.
    this.host.signIn = async () => {
      await this.beginSignIn();
      return this.signInPhase() !== "Failed" && this.identity !== null;
    };
    // Which experiments are on: the day's `cua_app_active` carries them.
    this.telemetry?.record(d.native.appTelemetryExperimentsOn(this.settings.experiments));
  }

  /** Called with what changed; returns the unsubscribe. */
  subscribe(listener: (change: ModelChange) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  changed(change: ModelChange): void {
    for (const l of [...this.listeners]) l(change);
  }

  get servicesIn(): boolean {
    return this.servicesInNow();
  }

  /** This machine's CPU architecture, as the image catalog spells it. */
  get hostArch(): string {
    return this.native.appHostArch();
  }

  static nowMs(): bigint {
    return BigInt(Date.now());
  }

  // MARK: Launch

  /** The live services are in: read the account again and show what the backend has. */
  attachLive(): void {
    this.identity = this.account?.identity() ?? null;
    this.host.identity = this.identity;
    this.devices.signedIn = this.identity !== null;
    this.listWatchedSince = Date.now();
    this.changed("session");
    void (async () => {
      await this.refresh();
      await this.keyvault.refresh();
      await this.devices.refresh();
      this.changed("machines");
    })();
  }

  // MARK: Space list

  get spaces(): AppSpace[] {
    return this.roster.spaces;
  }

  get sidebar(): AppSidebarView {
    return this.native.appSidebar(this.spaces, "", this.selectedSpaceId ?? "");
  }

  /** The detail as this machine sees it (`appSpaceDetailFor`). */
  detail(space: AppSpace): AppSpaceDetail {
    return this.native.appSpaceDetailFor(space, this.usage.get(space.id), this.hostArch, this.settings.experiments, this.devices.accessNotice);
  }

  /** The Spaces whose desktop can be shown (running and reachable). */
  get streamableSpaceIds(): string[] {
    return this.spaces.filter((s) => this.detail(s).canStream).map((s) => s.id);
  }

  isDeleting(id: string): boolean {
    return this.native.appCreatesIsDeleting(this.creates, id);
  }

  send(action: AppRosterAction): void {
    this.roster = this.native.appRosterReduce(this.roster, action);
    this.changed("spaces");
  }

  select(id: string): void {
    this.selectedSpaceId = id;
    this.send(new this.native.AppRosterAction.Select({ id, now: AppModel.nowMs() }));
  }

  /** The roster: This machine first (when this app manages a host), then the registry's Spaces. */
  recompose(): void {
    const listed = this.native.appWithoutThisRelayMachine(this.registrySpaces, this.host.machineId ?? undefined);
    const spaces = this.host.host === null ? listed : this.native.appWithThisMachine(listed, this.host.summaryInput, AppModel.nowMs());
    this.send(new this.native.AppRosterAction.SyncSpaces({ spaces: this.native.appCreatesCompose(spaces, this.creates) }));
  }

  /** Advances the Spaces being created and redraws the list. */
  sendCreate(action: AppCreateAction): void {
    // A create started, reached ready or failed (how long it took).
    this.telemetry?.record(this.native.appTelemetryCreates(this.creates, action, AppModel.nowMs()));
    this.creates = this.native.appCreatesReduce(this.creates, action);
    this.recompose();
    this.tickWhileCreating();
  }

  /** While a create runs, time moves its bar within a phase that reports no fraction (4 Hz). */
  private tickWhileCreating(): void {
    const running = this.creates.pending.some((p) => p.error === undefined && p.spaceId === undefined);
    if (!running) {
      if (this.createTicker) clearInterval(this.createTicker);
      this.createTicker = null;
      return;
    }
    if (this.createTicker) return;
    this.createTicker = setInterval(() => {
      const now = AppModel.nowMs();
      const tick = new this.native.AppCreateAction.Tick({ now });
      this.telemetry?.record(this.native.appTelemetryCreates(this.creates, tick, now));
      this.creates = this.native.appCreatesReduce(this.creates, tick);
      this.recompose();
      if (!this.creates.pending.some((p) => p.error === undefined && p.spaceId === undefined)) this.tickWhileCreating();
    }, TICK_MS);
    this.createTicker.unref?.();
  }

  // MARK: List health

  /** The list poll: a bounded refresh every `intervalMs`, and a reconnect when the list went stale. Returns the stop. */
  startListPoll(intervalMs = 10_000): () => void {
    let stopped = false;
    void (async () => {
      while (!stopped) {
        await this.refresh();
        await this.checkListHealth();
        await sleep(intervalMs);
      }
    })();
    return () => {
      stopped = true;
    };
  }

  async checkListHealth(now = Date.now()): Promise<void> {
    if (!this.startup.isReady || !this.reconnect || this.reconnecting) return;
    const since = Math.max(this.lastListOk ?? this.listWatchedSince, this.listWatchedSince);
    if (now - since < this.listStaleAfterMs) return;
    await this.reconnectNow(`the Space list was not read for ${Math.round((now - since) / 1000)} s`);
  }

  /** A new connection to the daemon, then the list again. One at a time. */
  async reconnectNow(reason: string): Promise<void> {
    if (!this.reconnect || this.reconnecting) return;
    this.reconnecting = true;
    this.reconnects += 1;
    console.log(`[cua-spaces] reconnecting to the cua daemon (${reason})`);
    this.rowsInFlight = null;
    this.hostInFlight = null;
    const ok = await this.reconnect();
    this.listWatchedSince = Date.now();
    this.reconnecting = false;
    if (!ok) console.log("[cua-spaces] the reconnect did not make a new connection; trying again later");
    await this.refresh();
  }

  private async refreshHost(): Promise<void> {
    const task =
      this.hostInFlight ??
      (async () => {
        await this.host.refresh();
        // A session that expired for good (or `cua auth logout`) pauses relay sharing too.
        await this.host.reconcileAccount(ACCOUNT_CHECK_MS);
      })();
    this.hostInFlight = task;
    const done = await withTimeout(this.hostTimeout, () => task);
    if (done.ok && this.hostInFlight === task) this.hostInFlight = null;
  }

  private async readRows() {
    const task = this.rowsInFlight ?? this.backend.rows();
    this.rowsInFlight = task;
    const result = await withTimeout(this.listTimeout, () => task);
    // Still running after the timeout: the next refresh waits on the same read.
    if (!(!result.ok && result.error instanceof TimeoutError) && this.rowsInFlight === task) this.rowsInFlight = null;
    if (!result.ok) throw result.error;
    return result.value;
  }

  /** Re-reads the registry (and the host). Bounded: a daemon that stopped answering leaves the list as it was. */
  async refresh(): Promise<void> {
    await this.refreshHost();
    const cloud = await this.backend.cloudAvailable().catch(() => false);
    if (cloud !== this.cloudConfigured) {
      this.cloudConfigured = cloud;
      this.changed("session");
    }
    try {
      const rows = await this.readRows();
      this.lastListOk = Date.now();
      this.registrySpaces = this.native.appRowsToSpaces(rows, AppModel.nowMs());
      this.creates = this.native.appCreatesSettle(this.creates, this.registrySpaces);
      this.recompose();
      this.loaded = true;
      this.rosterError = null;
      if (this.selectedSpaceId === null) {
        const first = this.sidebar.selectedId;
        if (first) this.selectedSpaceId = first;
      }
      this.changed("spaces");
      // A preview for every running Space from the daemon's cache (it
      // survives restarts), before one is opened.
      void this.thumbnails.warm(this.streamableSpaceIds);
    } catch (error) {
      // The daemon is slow or gone: the list stays, and the poll reconnects when it stays stale.
      if (error instanceof TimeoutError) return;
      this.rosterError = this.loaded ? "Could not refresh Spaces. Previously loaded rows may be out of date." : "Could not load Spaces. Try refreshing again.";
      this.changed("spaces");
    }
  }

  // MARK: New Space

  /** What New Space knows, freshly probed; every probe runs at once and each is bounded. */
  async newSpaceEnv(): Promise<AppWizardEnv> {
    const b = this.backend;
    const [runtimes, storage, pricing, gpus, hosts, cloud, lume, linux, vms] = await Promise.all([
      b.localRuntimes(),
      b.localStorage(),
      b.cloudPricing(),
      b.gpuChoices(),
      b.hosts(),
      b.cloudAvailable(),
      b.lumeSource(),
      b.linuxSource(),
      b.runningMacosVms(),
      this.cloud.refresh(),
    ]);
    this.lumeSource = lume;
    this.linuxSource = linux;
    this.hosts = hosts;
    // Lume did not answer this time: keep what it said last.
    this.runningMacosVms = vms ?? this.runningMacosVms;
    return this.wizardEnv(cloud, runtimes, storage, pricing, gpus);
  }

  /** What New Space knows without asking anything: the env while the live services are still starting. */
  knownNewSpaceEnv(): AppWizardEnv {
    return this.wizardEnv(false, null, null, null, null);
  }

  private wizardEnv(
    available: boolean,
    runtimes: LocalRuntimes | null,
    storage: LocalStorage | null,
    pricing: AppCloudPricing | null,
    gpus: AppGpuChoice[] | null,
  ): AppWizardEnv {
    const backends = runtimes?.ready;
    const env: AppWizardEnv = {
      defaultLocation: this.settings.defaultLocation,
      cloudAvailable: available,
      localAvailable: backends ? backends.length > 0 : true,
      localReason: backends && backends.length === 0 ? "No local runtime found (Docker or Lume)." : undefined,
      localBackends: backends,
      localDetails: runtimes?.details,
      maxCpus: this.maxCpus,
      hostArch: this.hostArch,
      lumeSource: this.lumeSource ?? undefined,
      linuxSource: this.linuxSource ?? undefined,
      storage: storage ? wizardStorage(storage) : undefined,
      cloudPricing: available ? (pricing ?? undefined) : undefined,
      clouds: this.cloud.clouds,
      hosts: this.hosts,
      experiments: this.settings.experiments,
      gpus: gpus ?? undefined,
    };
    if (this.native.appIsCloudWord(this.cloud.defaultOn ?? "")) env.defaultLocation = this.native.AppLocation.Yours;
    return env;
  }

  /**
   * Runs a create from the core's create arguments (New UI's `spaces.create`,
   * with the page's pending id): the pending row shows at once, follows the
   * SDK's progress (also handed to `progress`) and hands over to the
   * registry's row when it is ready; returns its id. A failure stays on its
   * row; a cancelled create only leaves the list. Either way it throws.
   */
  async runCreate(args: AppCreateSpaceArgs, os: AppSpaceOs, pendingId: string, progress?: (p: SpaceCreateProgress) => void): Promise<string> {
    this.startCreate(args, os, pendingId);
    return this.followCreate(args, pendingId, progress);
  }

  /** A new pending id (`pending:<uuid>`). */
  static pendingId(): string {
    return `pending:${randomUUID()}`;
  }

  private startCreate(args: AppCreateSpaceArgs, os: AppSpaceOs, pendingId: string): void {
    // A Space on this machine is reached over the local network (a macOS VM
    // on vmnet): ask now, while the person who pressed Create is here.
    if (args.on === "local") this.host.requestLocalNetwork();
    const machine = args.on.startsWith("host:") ? args.on.slice("host:".length) : undefined;
    const provider = args.on === "cloud" ? this.native.AppSpaceProvider.Cloud : args.on === "local" ? this.native.AppSpaceProvider.Local : this.native.AppSpaceProvider.Relay;
    this.sendCreate(
      new this.native.AppCreateAction.Start({
        id: pendingId,
        name: args.name ?? "",
        os,
        provider,
        now: AppModel.nowMs(),
        image: args.image,
        kind: args.kind,
        hostArch: this.hostArch,
        gpu: args.gpu !== undefined,
        host: machine,
        hostName: machine ? this.machineName(machine) : undefined,
      }),
    );
  }

  /** The name of one of your machines, from the Spaces it provides (or its own entry on the relay). */
  private machineName(id: string): string | undefined {
    return this.spaces.find((s) => s.id === `relay:${id}`)?.name ?? this.spaces.find((s) => s.host === id && s.hostName)?.hostName;
  }

  private async followCreate(args: AppCreateSpaceArgs, pendingId: string, progress?: (p: SpaceCreateProgress) => void): Promise<string> {
    try {
      let accepted = false;
      const create = this.backend.create(args, pendingId, (p) => {
        accepted = true;
        this.sendCreate(
          new this.native.AppCreateAction.Progress({
            id: pendingId,
            phase: p.phase,
            fraction: p.fraction,
            now: AppModel.nowMs(),
            bytesDone: p.bytesDone,
            bytesTotal: p.bytesTotal,
            bytesPerSecond: p.bytesPerSecond,
          }),
        );
        progress?.(p);
      });
      const id = await this.awaitAccepted(create, () => accepted, pendingId);
      this.sendCreate(new this.native.AppCreateAction.Finish({ id: pendingId, spaceId: id }));
      await this.refresh();
      if (this.selectedSpaceId === pendingId) this.select(id);
      // The registry lists it now: drop the pending row from the state too.
      this.sendCreate(new this.native.AppCreateAction.Dismiss({ id: pendingId }));
      return id;
    } catch (error) {
      if (isCancelled(error)) this.sendCreate(new this.native.AppCreateAction.CancelDone({ id: pendingId }));
      else this.sendCreate(new this.native.AppCreateAction.Fail({ id: pendingId, error: words(error) }));
      throw error;
    }
  }

  /** The create's result, or `CreateNotAccepted` when the daemon reported nothing within `createAcceptTimeout`. */
  private async awaitAccepted(create: Promise<string>, accepted: () => boolean, pendingId: string): Promise<string> {
    const first = await withTimeout(this.createAcceptTimeout, () => create);
    if (first.ok) return first.value;
    if (!(first.error instanceof TimeoutError)) throw first.error;
    if (accepted()) return create;
    console.log(`[cua-spaces] the cua daemon did not take create ${pendingId} within ${this.createAcceptTimeout} s`);
    // Best effort: it may still reach the daemon over the old connection; never wait on it.
    void withTimeout(30, () => this.backend.cancelCreate(pendingId));
    void this.reconnectNow("a create was not taken");
    create.catch(() => {});
    throw new CreateNotAccepted(Math.ceil(this.createAcceptTimeout));
  }

  /** "Connect by address": the SDK's handshake, then the list read again. */
  async addByAddress(url: string, token: string | null, name: string | null): Promise<void> {
    await this.backend.add(url, token, name);
    await this.refresh();
  }

  /** Cancel on a Space still being created: the row shows Cancelling until the SDK stopped it, then goes. */
  cancelCreate(pendingId: string): void {
    if (!this.native.appCreatesIsPending(pendingId) || !this.creates.pending.some((p) => p.id === pendingId && !p.cancelling)) return;
    this.sendCreate(new this.native.AppCreateAction.CancelStart({ id: pendingId }));
    void (async () => {
      try {
        await this.backend.cancelCreate(pendingId);
        this.sendCreate(new this.native.AppCreateAction.CancelDone({ id: pendingId }));
      } catch (error) {
        this.sendCreate(new this.native.AppCreateAction.CancelFail({ id: pendingId, error: words(error) }));
      }
    })();
  }

  /**
   * Delete (after the page's confirmation): the row shows Deleting at once,
   * and goes when the SDK's delete returns; a failure restores it with the
   * banner. `removeOnly` forgets the Space and keeps it running.
   */
  delete(space: AppSpace, removeOnly = false): void {
    // A failed create is only removed from the list.
    if (this.native.appCreatesIsPending(space.id)) {
      this.sendCreate(new this.native.AppCreateAction.Dismiss({ id: space.id }));
      return;
    }
    if (this.isDeleting(space.id)) return;
    this.recordFeature("space_delete");
    const detail = this.native.appSpaceDetail(space);
    this.sendCreate(new this.native.AppCreateAction.DeleteStart({ id: space.id, now: AppModel.nowMs() }));
    this.usage.delete(space.id);
    this.thumbnails.remove(space.id);
    void (async () => {
      try {
        await this.backend.remove(space.id, removeOnly || detail.removeOnly);
        this.sendCreate(new this.native.AppCreateAction.DeleteDone({ id: space.id }));
        await this.refresh();
      } catch (error) {
        this.sendCreate(new this.native.AppCreateAction.DeleteFail({ id: space.id }));
        this.show(this.native.appDeleteFailedText(space.name, words(error)), true);
      }
    })();
  }

  /** The power button: turns the Space off (suspended or stopped, as its provider can) or back on. */
  setPower(space: AppSpace, on: boolean): void {
    if (this.native.appCreatesIsPowering(this.creates, space.id)) return;
    this.sendCreate(new this.native.AppCreateAction.PowerStart({ id: space.id, on, now: AppModel.nowMs() }));
    if (!on) {
      this.usage.delete(space.id);
      this.thumbnails.remove(space.id);
    }
    void (async () => {
      try {
        await this.backend.setPower(space.id, on);
        this.sendCreate(new this.native.AppCreateAction.PowerDone({ id: space.id }));
        await this.refresh();
      } catch (error) {
        this.sendCreate(new this.native.AppCreateAction.PowerFail({ id: space.id, error: words(error) }));
      }
    })();
  }

  // MARK: Settings and banners

  saveSettings(): void {
    try {
      this.native.appSettingsSave(this.settingsPath, this.settings);
    } catch (error) {
      console.warn(`[cua-spaces] could not save ${this.settingsPath}: ${words(error)}`);
    }
    this.changed("settings");
  }

  /** The banner (the SwiftUI app's `show(error:)` and `show(info:)`); logged here, as the page has no banner. */
  show(text: string, isError: boolean): void {
    this.banner = text;
    this.bannerIsError = isError;
    console[isError ? "warn" : "log"](`[cua-spaces] ${text}`);
  }

  /** Records a Spaces app feature (a fixed name). */
  recordFeature(name: string): void {
    this.telemetry?.record(this.native.appTelemetryFeature(name));
  }

  /** The count the menu bar item and the notch show. */
  get statusLine(): string {
    return this.native.appStatusLine(this.native.appOpenableCount(this.spaces));
  }

  /** The window chrome (account line, New Space, empty state). */
  get chrome(): AppMainChrome {
    return this.native.appMainChrome({
      identity: this.identity ?? undefined,
      cloudConfigured: this.cloudConfigured,
      canSignIn: this.account !== null,
      experiments: this.settings.experiments,
    });
  }

  // MARK: Account

  /** "Sign in": the browser flow (opened by the account), the code while it waits. */
  async beginSignIn(): Promise<void> {
    if (!this.account || this.signInPhase() === "Starting") return;
    this.setSignIn(new this.native.AppSignInPhase.Starting());
    const current = ++this.signInAttempt;
    try {
      const attempt = await this.account.beginSignIn();
      // Cancelled while it started.
      if (this.signInAttempt !== current || this.signInPhase() !== "Starting") return;
      this.signInUrl = attempt.url;
      this.setSignIn(new this.native.AppSignInPhase.Waiting({ userCode: attempt.userCode ?? undefined }));
      const timeout = setTimeout(() => {
        if (this.signInAttempt === current && this.signInPhase() === "Waiting") this.signInFailed("The sign-in timed out. Try again.");
      }, this.signInTimeoutMs);
      timeout.unref?.();
      let who: string | null;
      try {
        who = await attempt.wait();
      } finally {
        clearTimeout(timeout);
      }
      if (this.signInAttempt !== current || this.signInPhase() !== "Waiting") return;
      this.identity = who ?? this.account.identity();
      this.host.identity = this.identity;
      this.devices.signedIn = this.identity !== null;
      this.signInUrl = null;
      this.setSignIn(new this.native.AppSignInPhase.Idle());
      // Relay sharing paused while signed out comes back for its owner.
      await this.host.reconcileAccount();
      await this.devices.refresh();
      this.changed("machines");
    } catch (error) {
      if (this.signInAttempt !== current || this.signInPhase() === "Idle") return;
      this.signInFailed(words(error));
    }
  }

  /** The sign-in's phase (`Idle`, `Starting`, `Waiting`, `Failed`). */
  signInPhase(): string {
    return this.signIn.tag;
  }

  private setSignIn(phase: AppSignInPhase): void {
    this.signIn = phase;
    this.changed("session");
  }

  /** Shown where the sign-in was started; counted by its kind only. */
  private signInFailed(message: string): void {
    this.signInUrl = null;
    this.setSignIn(new this.native.AppSignInPhase.Failed({ message }));
    this.telemetry?.record(this.native.appTelemetrySignInFailed(message));
  }

  /** Stops waiting for the browser (the page can start again). */
  cancelSignIn(): void {
    if (this.signInPhase() === "Starting" || this.signInPhase() === "Waiting") {
      this.signInAttempt += 1;
      this.telemetry?.record(this.native.appTelemetrySignInFailed(undefined));
    }
    this.signInUrl = null;
    this.setSignIn(new this.native.AppSignInPhase.Idle());
  }

  async signOut(): Promise<void> {
    try {
      await this.account?.signOut();
    } catch (error) {
      console.warn(`[cua-spaces] sign out: ${words(error)}`);
    }
    this.identity = null;
    this.host.identity = null;
    this.devices.signedIn = false;
    // Signed out: relay sharing stops (the setup stays to resume).
    await this.host.reconcileAccount();
    await this.devices.refresh();
    this.setSignIn(new this.native.AppSignInPhase.Idle());
    this.changed("machines");
  }
}

/** The SDK's storage probe as the wizard env takes it. */
export function wizardStorage(s: LocalStorage): AppLocalStorage {
  const volume = (v: LocalStorage["lume"]) => (v ? { availableBytes: v.availableBytes, totalBytes: v.totalBytes, name: v.name } : undefined);
  return { reserveBytes: s.reserveBytes, lume: volume(s.lume), qemu: volume(s.qemu), container: volume(s.container), pulled: s.pulled };
}
