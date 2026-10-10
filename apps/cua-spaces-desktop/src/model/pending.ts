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
  TeleportLike,
} from "../native/generated/index";
import type { LiveSpacesBackend, SpacesBackend } from "./backend";
import type { AgentSetupRunning } from "./agents";
import type { AccountProfile, AccountRunning, SignInAttempt } from "./services";

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
  create = async (args: AppCreateSpaceArgs, createId: string, progress: (p: SpaceCreateProgress) => void) =>
    (await this.b()).create(args, createId, progress);
  cancelCreate = async (createId: string) => (await this.b()).cancelCreate(createId);
  gpuChoices = async () => (await this.b()).gpuChoices();
  hosts = async () => (await this.b()).hosts();
  reportedHostname = (id: string) => this.current?.reportedHostname(id) ?? null;
  add = async (url: string, token: string | null, name: string | null) => (await this.b()).add(url, token, name);
  remove = async (id: string, removeOnly: boolean) => (await this.b()).remove(id, removeOnly);
  setPower = async (id: string, on: boolean) => (await this.b()).setPower(id, on);
  localRuntimes = async () => (await this.b()).localRuntimes();
  lumeSource = async () => (await this.b()).lumeSource();
  setLumeSource = async (value: string) => (await this.b()).setLumeSource(value);
  linuxSource = async () => (await this.b()).linuxSource();
  setLinuxSource = async (value: string) => (await this.b()).setLinuxSource(value);
  localStorage = async () => (await this.b()).localStorage();
  cloudAvailable = async () => (await this.b()).cloudAvailable();
  runningMacosVms = async () => (await this.b()).runningMacosVms();
  cloudPricing = async () => (await this.b()).cloudPricing();
  usage = async (id: string) => (await this.b()).usage(id);
  agentRuns = async (id: string) => (await this.b()).agentRuns(id);
  windows = async (id: string) => (await this.b()).windows(id);
  primaryDisplay = async (id: string) => (await this.b()).primaryDisplay(id);
  thumbnail = async (id: string, maxAgeMs: number | null) => (await this.b()).thumbnail(id, maxAgeMs);
  sendFiles = async (id: string, paths: string[]) => (await this.b()).sendFiles(id, paths);
  shares = async (id: string) => (await this.b()).shares(id);
  share = async (id: string, who: string, role: string) => (await this.b()).share(id, who, role);
  unshare = async (id: string, who: string) => (await this.b()).unshare(id, who);
  tool = async (tool: string, args: Record<string, unknown>) => (await this.b()).tool(tool, args);
  teleportContext = async (id: string): Promise<{ teleport: TeleportLike; space: SpaceLike } | null> => (await this.b()).teleportContext(id);
  teleportHandle = () => this.current?.teleportHandle() ?? null;
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

  async beginSignIn(): Promise<SignInAttempt> {
    const account = (await this.gate.wait()).account;
    if (!account) throw new NotStarted("Signing in");
    return account.beginSignIn();
  }

  async signOut(): Promise<void> {
    await (await this.gate.wait()).account?.signOut();
  }

  async accessToken(force: boolean): Promise<string | null> {
    const account = (await this.gate.wait()).account;
    return account ? account.accessToken(force) : null;
  }
}
