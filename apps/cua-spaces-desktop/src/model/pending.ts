// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Stand-ins that wait for the live services (the SwiftUI app's `Pending*`
// in AppStartup.swift): the window opens at once, and every call waits until
// the launch made the SDK; a reconnect swaps in a new backend.
import type {
  AppCreateSpaceArgs,
  AppSpaceAgentRun,
  CuaLike,
  DevicesLike,
  HostLike,
  SpaceAppIconRequest,
  SpaceCreateProgress,
  SpaceLike,
  PresenceIdentity,
  TeleportLike,
} from "../native/generated/index";
import type { LiveSpacesBackend, ShareSettingsRow, SpacesBackend, TcpShareOptions } from "./backend";
import type { AgentSetupRunning } from "./agents";
import type { AccountProfile, AccountRunning, SignInAttempt, SignInOptions } from "./services";

/** A value set once, which callers wait for. */
export class LiveGate<T> {
  private value: T | null = null;
  private readonly waiters: ((v: T) => void)[] = [];

  get current(): T | null {
    return this.value;
  }

  /** Sets it (once; later calls are ignored, answering false) and wakes every waiter. */
  resolve(v: T): boolean {
    if (this.value !== null) return false;
    this.value = v;
    for (const w of this.waiters.splice(0)) w(v);
    return true;
  }

  wait(): Promise<T> {
    if (this.value !== null) return Promise.resolve(this.value);
    return new Promise((resolve) => this.waiters.push(resolve));
  }
}

/** Why a live service is missing (the SDK did not start). */
export class NotStarted extends Error {
  constructor(what: string) {
    super(`${what} needs Cua, which did not start. Reopen Cua Spaces to try again.`);
    this.name = "NotStarted";
  }
}

/** What the launch made: the SDK backend and what hangs off it, or why it could not start. */
export interface LiveServices {
  backend: SpacesBackend | null;
  live: LiveSpacesBackend | null;
  cua: CuaLike | null;
  account: AccountRunning | null;
  /** This machine's host service (`Host`). */
  host: HostLike | null;
  /** The coding agents on this machine (the SDK's agent onboarding). */
  agentSetup: AgentSetupRunning | null;
  /** This machine on the relay, for the signed-in session (`Devices`). */
  devices: DevicesLike | null;
  startError: string | null;
  daemonError: string | null;
}

/** The Spaces backend until the SDK is in. */
export class PendingSpacesBackend implements SpacesBackend {
  private replaced: SpacesBackend | null = null;

  constructor(readonly gate: LiveGate<LiveServices>) {}

  /** Calls from now on go to `backend` (a fresh client of the daemon). */
  replace(backend: SpacesBackend): void {
    this.replaced = backend;
  }

  /** The backend calls go to now (null until the SDK is in). */
  get current(): SpacesBackend | null {
    return this.replaced ?? this.gate.current?.backend ?? null;
  }

  private async b(): Promise<SpacesBackend> {
    if (this.replaced) return this.replaced;
    const b = (await this.gate.wait()).backend;
    if (!b) throw new NotStarted("This");
    return b;
  }

  rows = async () => (await this.b()).rows();
  listRead = async () => {
    const b = await this.b();
    return b.listRead ? b.listRead() : { rows: await b.rows(), incomplete: false, reaching: 0, stale: false, notice: "" };
  };
  create = async (args: AppCreateSpaceArgs, createId: string, progress: (p: SpaceCreateProgress) => void) => (await this.b()).create(args, createId, progress);
  cancelCreate = async (createId: string) => (await this.b()).cancelCreate(createId);
  gpuChoices = async () => (await this.b()).gpuChoices();
  hosts = async () => (await this.b()).hosts();
  reportedHostname = (id: string) => this.current?.reportedHostname(id) ?? null;
  add = async (url: string, token: string | null, name: string | null) => (await this.b()).add(url, token, name);
  remove = async (id: string, removeOnly: boolean) => (await this.b()).remove(id, removeOnly);
  deleteMany = async (ids: string[]) => (await this.b()).deleteMany(ids);
  leave = async (ids: string[]) => (await this.b()).leave(ids);
  rename = async (id: string, name: string) => (await this.b()).rename(id, name);
  relayUnregister = async (machineId: string, deleteSpaces?: boolean) => (await this.b()).relayUnregister(machineId, deleteSpaces);
  network = async (id: string) => {
    const b = await this.b();
    if (!b.network) throw new Error("network settings are not available in this build");
    return b.network(id);
  };
  setNetwork = async (id: string, word: string, allow: string[], deny: string[], widen: boolean, tree: boolean, via?: string) => {
    const b = await this.b();
    if (!b.setNetwork) throw new Error("network settings are not available in this build");
    return b.setNetwork(id, word, allow, deny, widen, tree, via);
  };
  exitsThroughThisMachine = async () => {
    const b = await this.b();
    return b.exitsThroughThisMachine ? b.exitsThroughThisMachine() : [];
  };
  networkLog = async (id: string) => {
    const b = await this.b();
    return b.networkLog ? b.networkLog(id) : [];
  };
  ackNetworkNotice = async (id: string) => {
    const b = await this.b();
    await b.ackNetworkNotice?.(id);
  };
  setAccess = async (id: string, clipboard: string | null, volume: string | null) => {
    const b = await this.b();
    if (!b.setAccess) throw new Error("these settings are not available in this build");
    await b.setAccess(id, clipboard, volume);
  };
  machineHealth = async (machine: string | null) => (await this.b()).machineHealth(machine);
  setPower = async (id: string, on: boolean) => (await this.b()).setPower(id, on);
  localRuntimes = async () => (await this.b()).localRuntimes();
  lumeSource = async () => (await this.b()).lumeSource();
  setLumeSource = async (value: string) => (await this.b()).setLumeSource(value);
  linuxSource = async () => (await this.b()).linuxSource();
  setLinuxSource = async (value: string) => (await this.b()).setLinuxSource(value);
  runtimeKinds = async () => (await this.b()).runtimeKinds();
  runtimeCheckPath = async (kind: string, path: string) => (await this.b()).runtimeCheckPath(kind, path);
  setRuntimeSetting = async (key: string, value: string | null) => (await this.b()).setRuntimeSetting(key, value);
  repoCacheStatus = async () => (await this.b()).repoCacheStatus();
  setRepoCache = async (change: { enabled?: boolean; sizeGb?: number | null }) => (await this.b()).setRepoCache(change);
  clearRepoCache = async () => (await this.b()).clearRepoCache();
  localStorage = async () => (await this.b()).localStorage();
  loggedIn = async () => (await this.b()).loggedIn();
  runningMacosVms = async () => (await this.b()).runningMacosVms();
  usage = async (id: string) => (await this.b()).usage(id);
  // Never waits for the SDK: until it is in, the page shows this
  // computer's own numbers.
  resources = async (visible: boolean) => (await this.current?.resources?.(visible)) ?? null;
  agentRuns = async (id: string) => (await this.b()).agentRuns(id);
  windows = async (id: string) => (await this.b()).windows(id);
  primaryDisplay = async (id: string) => (await this.b()).primaryDisplay(id);
  thumbnail = async (id: string, maxAgeMs: number | null) => (await this.b()).thumbnail(id, maxAgeMs);
  sendFiles = async (id: string, paths: string[]) => (await this.b()).sendFiles(id, paths);
  shares = async (id: string) => (await this.b()).shares(id);
  share = async (id: string, who: string, role: string) => (await this.b()).share(id, who, role);
  unshare = async (id: string, who: string) => (await this.b()).unshare(id, who);
  links = async () => (await this.b()).links();
  createLink = async (target: string, access: string | null, ttlSeconds: number | null, name: string | null) =>
    (await this.b()).createLink(target, access, ttlSeconds, name);
  createTcpLink = async (target: string, service: string | null, ttlSeconds: number | null, name: string | null, options?: TcpShareOptions) => {
    const b = await this.b();
    if (!b.createTcpLink) throw new Error("Public TCP shares are not available here");
    return b.createTcpLink(target, service, ttlSeconds, name, options);
  };
  approveLink = async (id: string, always: boolean) => {
    const b = await this.b();
    if (!b.approveLink) throw new Error("Public TCP shares are not available here");
    return b.approveLink(id, always);
  };
  denyLink = async (id: string) => {
    const b = await this.b();
    if (!b.denyLink) throw new Error("Public TCP shares are not available here");
    return b.denyLink(id);
  };
  shareSettings = async () => {
    const b = await this.b();
    if (!b.shareSettings) throw new Error("Public TCP shares are not available here");
    return b.shareSettings();
  };
  setShareSettings = async (s: ShareSettingsRow) => {
    const b = await this.b();
    if (!b.setShareSettings) throw new Error("Public TCP shares are not available here");
    return b.setShareSettings(s);
  };
  removeShareEdge = async () => {
    const b = await this.b();
    if (!b.removeShareEdge) throw new Error("Public TCP shares are not available here");
    return b.removeShareEdge();
  };
  stopLink = async (id: string) => (await this.b()).stopLink(id);
  tool = async (tool: string, args: Record<string, unknown>) => (await this.b()).tool(tool, args);
  teleportContext = async (id: string): Promise<{ teleport: TeleportLike; space: SpaceLike } | null> => (await this.b()).teleportContext(id);
  teleportHandle = () => this.current?.teleportHandle() ?? null;
  joinPresence = async (id: string, identity: PresenceIdentity, timeoutMs: number) => (await this.b()).joinPresence(id, identity, timeoutMs);
  appIcons = async (id: string, requests: SpaceAppIconRequest[]) => (await this.b()).appIcons(id, requests);
}

/** The account until the SDK is in: no identity yet; sign-in waits. */
export class PendingAccount implements AccountRunning {
  constructor(private readonly gate: LiveGate<LiveServices>) {}

  identity(): string | null {
    return this.gate.current?.account?.identity() ?? null;
  }

  profile(): AccountProfile | null {
    return this.gate.current?.account?.profile() ?? null;
  }

  async beginSignIn(opts?: SignInOptions): Promise<SignInAttempt> {
    const account = (await this.gate.wait()).account;
    if (!account) throw new NotStarted("Signing in");
    return account.beginSignIn(opts);
  }

  async signOut(): Promise<void> {
    await (await this.gate.wait()).account?.signOut();
  }

  async accessToken(force: boolean): Promise<string | null> {
    const account = (await this.gate.wait()).account;
    return account ? account.accessToken(force) : null;
  }
}
