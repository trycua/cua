// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Synthetic Spaces, host, devices, account and telemetry for the bridge's
// tests on the real app core (the SwiftUI app's FixtureSpacesBackend,
// FixtureHost, FixtureDevices, FixtureAccount and FixtureTelemetry). Never
// contacts anything.
import type { Native } from "../src/native/load";
import type {
  AppCreateSpaceArgs,
  AppTelemetryInput,
  AppTelemetrySignal,
  AppSpaceRow,
  DevicesLike,
  DevicesSnapshot,
  HostLike,
  HostStatus,
  SpaceAppIconRequest,
  SpaceCreateProgress,
  SpaceLike,
  TeleportLike,
} from "../src/native/generated/index";
import type { LocalRuntimes, SpaceThumbnailData, SpacesBackend } from "../src/model/backend";
import type { StreamWindow } from "../src/model/streams";
import type { AccountProfile, AccountRunning, SignInAttempt, TelemetryRunning } from "../src/model/services";

export function sampleRows(n: Native): AppSpaceRow[] {
  const row = (r: Partial<AppSpaceRow> & Pick<AppSpaceRow, "id" | "name" | "provider">): AppSpaceRow => ({
    spacesdVersion: "0.4.0",
    features: [],
    addedAt: undefined,
    os: undefined,
    osName: undefined,
    osPrettyName: undefined,
    image: undefined,
    imageDigest: undefined,
    kind: undefined,
    arch: undefined,
    reachable: true,
    error: undefined,
    host: undefined,
    hostName: undefined,
    power: undefined,
    powerState: undefined,
    cloud: undefined,
    cloudPlace: undefined,
    cloudDelete: undefined,
    ...r,
  });
  return [
    row({
      id: "local:aurora",
      name: "0123456789ab",
      provider: "local",
      features: ["desktop_stream", "window_stream"],
      addedAt: "2026-09-25T08:00:00Z",
      os: n.AppSpaceOs.Linux,
      osName: "Ubuntu",
      osPrettyName: "Ubuntu 24.04.3 LTS",
      image: "ghcr.io/trycua/linux:24.04",
      kind: n.AppSpaceKind.Container,
      arch: "arm64",
      power: "suspend",
      powerState: "running",
    }),
    row({ id: "cloud:builder", name: "builder", provider: "cloud", features: ["desktop_stream"], addedAt: "2026-09-24T08:00:00Z", os: n.AppSpaceOs.Linux, osName: "Arch Linux", reachable: false, error: "timed out" }),
    row({ id: "direct:10.0.0.5:3211", name: "10.0.0.5:3211", provider: "direct", addedAt: "2026-09-23T08:00:00Z", os: n.AppSpaceOs.Windows, osName: "Windows" }),
  ];
}

export class FixtureSpacesBackend implements SpacesBackend {
  rowsNow: AppSpaceRow[];
  readonly created: AppCreateSpaceArgs[] = [];
  readonly removed: { id: string; removeOnly: boolean }[] = [];
  readonly power: { id: string; on: boolean }[] = [];
  readonly cancelled = new Set<string>();
  /** Progress a fixture create reports, in order (the SDK's words). */
  phases: Omit<SpaceCreateProgress, "space">[] = [
    { phase: "preparing", fraction: undefined, detail: "", bytesDone: undefined, bytesTotal: undefined, bytesPerSecond: undefined },
    { phase: "pulling", fraction: 0.5, detail: "ghcr.io/trycua/linux:24.04", bytesDone: 5n, bytesTotal: 10n, bytesPerSecond: 2.5 },
    { phase: "ready", fraction: undefined, detail: "", bytesDone: undefined, bytesTotal: undefined, bytesPerSecond: undefined },
  ];
  /** Holds each create until `release` (to cancel it meanwhile). */
  hold: Promise<void> | null = null;
  createError: Error | null = null;

  constructor(private readonly native: Native) {
    this.rowsNow = sampleRows(native);
  }

  async rows() {
    return this.rowsNow;
  }

  async create(args: AppCreateSpaceArgs, createId: string, progress: (p: SpaceCreateProgress) => void) {
    this.created.push(args);
    for (const p of this.phases.slice(0, -1)) progress({ ...p, space: createId });
    if (this.hold) await this.hold;
    if (this.cancelled.has(createId)) {
      const e = new Error(`CuaError.Cancelled: Cancelled ${createId}; removed what it made.`);
      throw Object.assign(e, { [Symbol.for("typeName")]: "CuaError", tag: "Cancelled" });
    }
    if (this.createError) throw this.createError;
    progress({ ...this.phases.at(-1)!, space: createId });
    const id = `local:${args.name ?? `space-${this.created.length}`}`;
    this.rowsNow = [
      ...this.rowsNow,
      { ...this.rowsNow[0]!, id, name: args.name ?? "", image: args.image, provider: "local", powerState: undefined, power: undefined },
    ];
    return id;
  }

  async cancelCreate(createId: string) {
    this.cancelled.add(createId);
  }

  async gpuChoices() {
    return [];
  }
  async hosts() {
    return [];
  }
  reportedHostname() {
    return null;
  }
  readonly added: { url: string; token: string | null; name: string | null }[] = [];
  /** A Space at an address: listed once added. */
  async add(url: string, token: string | null, name: string | null) {
    this.added.push({ url, token, name });
    const id = `direct:${url.replace(/^\w+:\/\//, "")}`;
    this.rowsNow = [...this.rowsNow, { ...this.rowsNow[2]!, id, name: name ?? url.replace(/^\w+:\/\//, "") }];
  }
  async remove(id: string, removeOnly: boolean) {
    this.removed.push({ id, removeOnly });
    this.rowsNow = this.rowsNow.filter((r) => r.id !== id);
  }
  async setPower(id: string, on: boolean) {
    this.power.push({ id, on });
    this.rowsNow = this.rowsNow.map((r) => (r.id === id ? { ...r, powerState: on ? "running" : r.power === "stop" ? "stopped" : "suspended", reachable: on } : r));
  }
  async localRuntimes(): Promise<LocalRuntimes> {
    return { ready: ["docker"], details: new Map([["lume", "not installed"]]) };
  }
  async lumeSource() {
    return "auto";
  }
  async setLumeSource() {}
  async linuxSource() {
    return "auto";
  }
  async setLinuxSource() {}
  async localStorage() {
    return null;
  }
  async cloudAvailable() {
    return false;
  }
  async runningMacosVms() {
    return 1;
  }
  async cloudPricing() {
    return null;
  }
  async usage() {
    return null;
  }
  /** Run records (`agent_list` JSON), per Space id. */
  fixtureRuns: Record<string, string[]> = {};
  agentRunsError: Error | null = null;
  async agentRuns(id: string) {
    if (this.agentRunsError) throw this.agentRunsError;
    return this.native.appAgentRows(this.fixtureRuns[id] ?? []);
  }
  /** The fixture Space's windows (an empty list for any other). */
  windowsNow: StreamWindow[] = [
    { id: "w-1", app: "Terminal", title: "cua@space: ~", epoch: 3, width: 1280, height: 800, appId: "org.gnome.Terminal", pid: 412 },
    { id: "w-2", app: "Thunar", title: "", epoch: 1, width: 0, height: 0, appId: "", pid: 0 },
  ];
  windowsError: Error | null = null;
  async windows(id: string): Promise<StreamWindow[]> {
    if (this.windowsError) throw this.windowsError;
    return id === "local:aurora" ? this.windowsNow : [];
  }
  async primaryDisplay() {
    return { widthPx: 1920, heightPx: 1080 };
  }
  /** Thumbnail asks, in order (`[id, maxAgeMs]`). */
  readonly thumbnailCalls: [string, number | null][] = [];
  thumbnails = new Map<string, SpaceThumbnailData>();
  async thumbnail(id: string, maxAgeMs: number | null) {
    this.thumbnailCalls.push([id, maxAgeMs]);
    return this.thumbnails.get(id) ?? null;
  }
  readonly sent: { id: string; paths: string[] }[] = [];
  async sendFiles(id: string, paths: string[]) {
    this.sent.push({ id, paths });
    return paths.map((p) => ({ name: p.split("/").pop() ?? p, dest: `/home/cua/Desktop/${p.split("/").pop()}`, bytes: 1024n }));
  }
  /** Who each Space is shared with. */
  sharesNow = new Map<string, { who: string; role: string; connected: boolean }[]>();
  async shares(id: string) {
    return this.sharesNow.get(id) ?? [];
  }
  async share(id: string, who: string, role: string) {
    const rows = (this.sharesNow.get(id) ?? []).filter((r) => r.who !== who);
    this.sharesNow.set(id, [...rows, { who, role, connected: false }]);
    return this.shares(id);
  }
  async unshare(id: string, who: string) {
    this.sharesNow.set(id, (this.sharesNow.get(id) ?? []).filter((r) => r.who !== who));
    return this.shares(id);
  }
  /** The daemon's tool calls, in order. */
  readonly toolCalls: [string, Record<string, unknown>][] = [];
  cloudConnected = false;
  async tool(tool: string, args: Record<string, unknown> = {}): Promise<unknown> {
    this.toolCalls.push([tool, args]);
    const aws = (connected: boolean) => ({
      name: "aws",
      title: "AWS",
      tier: "vm",
      connected,
      credentials: { found: true, source: "~/.aws profile default" },
      region: "us-west-2",
      label: "AWS \u00B7 us-west-2",
      ttl_hours: 8,
      kinds: [
        { image: "linux", kind: "container", supported: true, machine_type: "t4g.medium", usd_per_hour: 0.0368 },
        { image: "windows", kind: "vm", supported: false, reason: "Windows on AWS is not offered yet." },
      ],
    });
    if (tool === "cloud_status") return { default_on: this.cloudConnected ? "aws" : "local", providers: [aws(this.cloudConnected)] };
    if (tool === "cloud_test") return { provider: "aws", ok: true, account: "1", checks: [{ name: "credentials", ok: true, detail: "account 1" }] };
    if (tool === "cloud_connect") {
      this.cloudConnected = true;
      return { ...aws(true), checks: [{ name: "credentials", ok: true, detail: "account 1" }] };
    }
    throw new Error(`unknown tool ${tool}`);
  }

  /** The SDK's teleport handle and Space (a test sets them; none: Teleport needs a reachable Space). */
  teleport: TeleportLike | null = null;
  space: SpaceLike | null = null;
  /** The icons the Space answers, by app name. */
  guestIcons = new Map<string, Uint8Array>();
  readonly iconRequests: SpaceAppIconRequest[][] = [];
  async teleportContext() {
    return this.teleport && this.space ? { teleport: this.teleport, space: this.space } : null;
  }
  teleportHandle() {
    return this.teleport;
  }
  async appIcons(_id: string, requests: SpaceAppIconRequest[]) {
    this.iconRequests.push(requests);
    return requests.map((r) => this.guestIcons.get(r.appName) ?? null);
  }
}

export function unconfiguredHost(): HostStatus {
  return {
    configured: false,
    mode: undefined,
    relayUrl: undefined,
    directUrl: undefined,
    envTokenPath: undefined,
    machineId: undefined,
    name: undefined,
    sharing: false,
    serviceInstalled: false,
    serviceRunning: false,
    serviceKind: "process",
    online: undefined,
    clients: [],
    allow: [],
    permissions: [],
    error: undefined,
    recentAccess: [],
    accessLogError: undefined,
    shareDesktop: true,
    provideSpaces: false,
    maxSpaces: 4,
    maxMacosVms: 2,
    providedSpaces: [],
    spacesAudit: [],
    spacesAuditError: undefined,
    pausedSignedOut: false,
    owner: undefined,
    ownerEmail: undefined,
  };
}

/** An in-memory host: it never installs a service or touches the network. */
export function fixtureHost(status: HostStatus = unconfiguredHost()): HostLike {
  const current = { ...status };
  return {
    status: async () => current,
    pauseSignedOut: async () => current,
    resumeSignedIn: async () => current,
    startSharing: async () => current,
    stopSharing: async () => current,
    configure: async () => current,
    remove: async () => {},
    setup: async () => current,
  } as unknown as HostLike;
}

/** An account on a relay: this machine enrolled, a device asking, and a machine online. */
export function fixtureDevices(now = BigInt(Math.floor(Date.now() / 1000))): DevicesLike {
  const day = 86_400n;
  const device = (id: string, name: string, state: string, platform: string, until?: bigint, seen?: bigint, current = false) => ({
    id,
    name,
    state,
    platform,
    createdAt: now - 40n * day,
    enrolledUntil: until,
    lastSeen: seen,
    current,
  });
  const snapshot: DevicesSnapshot = {
    localDeviceId: "dev_mac",
    devices: [
      device("dev_mac", "MacBook Pro", "enrolled", "macos", now + 24n * day, now - 60n, true),
      device("dev_work", "Work laptop", "pending", "windows"),
    ],
    enforceAfter: now - day,
    audit: [{ ts: now - 600n, kind: "machine_access", device: "dev_mac", machine: "m1", subject: undefined, detail: "owner" }],
    machineNames: new Map([["m1", "studio-mac"]]),
    machines: [
      { id: "m1", spaceId: "relay:m1", name: "studio-mac", ownerId: "me", ownerEmail: undefined, role: "owner", online: true, sharing: true, version: "0.5.0", url: "", allow: [], clients: [], confirmed: true },
    ],
  };
  return { snapshot: async () => snapshot } as unknown as DevicesLike;
}

/** Signing in answers with a device code, then the identity once `finish` runs. */
export class FixtureAccount implements AccountRunning {
  current: string | null;
  private finish: ((who: string) => void) | null = null;
  constructor(identity: string | null = null) {
    this.current = identity;
  }
  identity() {
    return this.current;
  }
  profile(): AccountProfile | null {
    return this.current ? { email: this.current } : null;
  }
  async beginSignIn(): Promise<SignInAttempt> {
    const done = new Promise<string>((resolve) => (this.finish = resolve));
    return {
      userCode: "ABCD-1234",
      url: "https://cua.ai/device",
      wait: async () => {
        this.current = await done;
        return this.current;
      },
    };
  }
  completeSignIn(who: string) {
    this.finish?.(who);
  }
  async signOut() {
    this.current = null;
  }
  async accessToken() {
    return this.current ? "token" : null;
  }
}

export class FixtureTelemetry implements TelemetryRunning {
  current: AppTelemetryInput = { enabled: true, lockedBy: undefined, noticeShown: false };
  readonly recorded: AppTelemetrySignal[] = [];
  status() {
    return this.current;
  }
  setEnabled(on: boolean) {
    this.current = { ...this.current, enabled: on };
    return this.current;
  }
  record(signals: AppTelemetrySignal[]) {
    if (this.current.enabled) this.recorded.push(...signals);
  }
}
