// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The platform side of Spaces: the cua SDK calls the app makes (the SwiftUI
// app's SpacesBackend.swift). Every decision about what they mean (names,
// status, grouping, what is allowed) is the app core's; this only moves
// data. Bounded where the app waits on it.
import type { Native } from "../native/load";
import type {
  AppCloudPricing,
  AppCreateSpaceArgs,
  AppGpuChoice,
  AppSpaceAgentRun,
  AppSpaceHost,
  AppSpaceKind,
  AppSpaceOs,
  AppSpaceRow,
  AppSentFileInfo,
  AppShareEntryInput,
  AppSpaceUsage,
  AppStreamDisplay,
  CuaLike,
  GpuSupport,
  LocalStorage,
  SpaceAppIconRequest,
  SpaceCreateProgress,
  SpaceShares,
  SpaceWindow,
  SpaceLike,
  TeleportLike,
} from "../native/generated/index";
import { words } from "./errors";
import type { StreamWindow } from "./streams";
import { bounded, withTimeout } from "./time";

export interface LocalRuntimes {
  /** The ready backends (lowercase names). */
  ready: string[];
  /** Why each one that is not ready is not (the check's detail). */
  details: Map<string, string>;
}

export interface SpacesBackend {
  /** The registry, one row per Space, with a bounded reachability probe. */
  rows(): Promise<AppSpaceRow[]>;
  /** Creates a Space, reporting the SDK's progress until it is ready; its id. */
  create(args: AppCreateSpaceArgs, createId: string, progress: (p: SpaceCreateProgress) => void): Promise<string>;
  cancelCreate(createId: string): Promise<void>;
  gpuChoices(): Promise<AppGpuChoice[] | null>;
  hosts(): Promise<AppSpaceHost[]>;
  /** The hostname a reachable Space's cua-spacesd reported at the last `rows()`. */
  reportedHostname(id: string): string | null;
  add(url: string, token: string | null, name: string | null): Promise<void>;
  remove(id: string, removeOnly: boolean): Promise<void>;
  setPower(id: string, on: boolean): Promise<void>;
  localRuntimes(): Promise<LocalRuntimes | null>;
  lumeSource(): Promise<string | null>;
  setLumeSource(value: string): Promise<void>;
  linuxSource(): Promise<string | null>;
  setLinuxSource(value: string): Promise<void>;
  localStorage(): Promise<LocalStorage | null>;
  cloudAvailable(): Promise<boolean>;
  runningMacosVms(): Promise<number | null>;
  cloudPricing(): Promise<AppCloudPricing | null>;
  usage(id: string): Promise<AppSpaceUsage | null>;
  /** The Space's coding-agent runs as rows (the core's mapping of the runs, attention first). */
  agentRuns(id: string): Promise<AppSpaceAgentRun[]>;
  /** The Space's windows that can be streamed (its Stream section; throws when they can't be read). */
  windows(id: string): Promise<StreamWindow[]>;
  /** The Space's primary display's size, bounded; null when unknown. */
  primaryDisplay(id: string): Promise<AppStreamDisplay | null>;
  /** The Space's latest thumbnail no older than `maxAgeMs` (null: any age), bounded; null when there is none. */
  thumbnail(id: string, maxAgeMs: number | null): Promise<SpaceThumbnailData | null>;
  /** Sends each path (a file or a folder) into the Space, in order. */
  sendFiles(id: string, paths: string[]): Promise<AppSentFileInfo[]>;
  /** Who the Space is shared with. */
  shares(id: string): Promise<AppShareEntryInput[]>;
  /** Shares it with `who` as `role`; who it is shared with now. */
  share(id: string, who: string, role: string): Promise<AppShareEntryInput[]>;
  /** Stops sharing it with `who`; who it is shared with now. */
  unshare(id: string, who: string): Promise<AppShareEntryInput[]>;
  /** One of the daemon's Spaces tools (`agent_*`, `volume_*`, `cloud_*`), its JSON answer. */
  tool(tool: string, args: Record<string, unknown>): Promise<unknown>;
  /** The SDK's teleport handle and the Space handle, for "Teleport an app"; null without a live SDK. Throws when the Space is unreachable. */
  teleportContext(id: string): Promise<{ teleport: TeleportLike; space: SpaceLike } | null>;
  /** The SDK's teleport handle, for drops (no Space needed). */
  teleportHandle(): TeleportLike | null;
  /** A Space's app icons in one call (`Space.appIcons`, through its one icon cache): 64 px PNG or SVG bytes, null where it has none. */
  appIcons(id: string, requests: SpaceAppIconRequest[]): Promise<(Uint8Array | null)[]>;
}

/** A Space's thumbnail as the SDK returns it: the encoded image (PNG or JPEG), its size and when it was captured. */
export interface SpaceThumbnailData {
  image: Uint8Array;
  format: "png" | "jpeg";
  width: number;
  height: number;
  capturedAtMs: number;
}

/** The SDK's share rows as the app core reads them (the SwiftUI app's `appShareRows`). */
export function shareRows(s: SpaceShares): AppShareEntryInput[] {
  return s.shares.map((e) => ({ who: e.who, role: e.role, connected: e.connected }));
}

/**
 * A window of the SDK's list as a stream target (the SwiftUI app's
 * `StreamWindow(SpaceWindow)`): its size from `bounds` (`[x, y, w, h]`), a
 * freshly listed window's epoch, and only windows with an id.
 */
export function streamWindows(windows: SpaceWindow[]): StreamWindow[] {
  return windows
    .filter((w) => w.windowId !== "")
    .map((w) => ({
      id: w.windowId,
      app: w.appName,
      title: w.title,
      epoch: Number(w.epoch) || 1,
      width: w.bounds.length === 4 ? Math.max(0, w.bounds[2]!) : 0,
      height: w.bounds.length === 4 ? Math.max(0, w.bounds[3]!) : 0,
      appId: w.appId,
      pid: Math.max(0, w.pid),
    }));
}

/** A tool's error, as the daemon worded it. */
export class ToolError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ToolError";
  }
}

/** How long a probe New Space waits for may take (s). */
export const PROBE_SECONDS = 10;

/** The SDK's `os` word as the core's; null when empty or unknown. */
export function osOf(native: Native, word: string): AppSpaceOs | undefined {
  switch (word.toLowerCase()) {
    case "macos":
    case "darwin":
      return native.AppSpaceOs.Macos;
    case "windows":
      return native.AppSpaceOs.Windows;
    case "linux":
      return native.AppSpaceOs.Linux;
    default:
      return undefined;
  }
}

export function kindOf(native: Native, word: string): AppSpaceKind | undefined {
  if (word === "container") return native.AppSpaceKind.Container;
  if (word === "vm") return native.AppSpaceKind.Vm;
  return undefined;
}

const text = (s: string | undefined) => (s ? s : undefined);

/** The supported feature names of a `GetCapabilities` answer; null when unreadable. */
export function supportedFeatures(capabilitiesJson: string | null): string[] | null {
  try {
    const caps = JSON.parse(capabilitiesJson ?? "") as { features?: { name?: unknown; supported?: unknown }[] };
    if (!Array.isArray(caps.features)) return null;
    return caps.features.filter((f) => f.supported === true && typeof f.name === "string").map((f) => f.name as string);
  } catch {
    return null;
  }
}

/** The guest hostname of a `GetCapabilities` answer. */
export function capabilitiesHostname(capabilitiesJson: string | null): string | null {
  try {
    const name = (JSON.parse(capabilitiesJson ?? "") as { hostname?: unknown }).hostname;
    return typeof name === "string" && name.trim() ? name.trim() : null;
  } catch {
    return null;
  }
}

/** The wizard's GPU choices: the first option of each runtime that has one. */
export function gpuChoices(support: GpuSupport[]): AppGpuChoice[] {
  return support.flatMap((s) => {
    const o = s.options[0];
    if (!o) return [];
    const reason = o.reason.trim();
    return [{ runtime: s.runtime, id: o.id, label: o.label, experimental: o.experimental, supported: o.supported, reason: reason || undefined, learnMore: o.learnMore }];
  });
}

/** Running macOS VMs in Lume's `GET /lume/vms`. */
export function runningMacosVms(vms: unknown): number | null {
  if (!Array.isArray(vms)) return null;
  return vms.filter((v) => {
    const vm = v as { os?: unknown; status?: unknown };
    return String(vm.os ?? "").toLowerCase() === "macos" && String(vm.status ?? "").toLowerCase() === "running";
  }).length;
}

/** The live backend: the cua SDK in this process, on this app's daemon when it runs, else embedded. */
export class LiveSpacesBackend implements SpacesBackend {
  private readonly hostnames = new Map<string, string>();
  private teleport: TeleportLike | null = null;

  constructor(
    private readonly native: Native,
    readonly cua: CuaLike,
    private readonly env: NodeJS.ProcessEnv = process.env,
    private readonly fetchImpl: typeof fetch = fetch,
    private readonly platform: NodeJS.Platform = process.platform,
  ) {}

  async rows(): Promise<AppSpaceRow[]> {
    const spaces = this.cua.spaces();
    const infos = await spaces.list();
    return Promise.all(
      infos.map(async (info) => {
        const row: AppSpaceRow = {
          id: info.id,
          name: info.name,
          provider: info.provider,
          spacesdVersion: info.spacesdVersion,
          features: info.features,
          addedAt: info.addedAt,
          os: osOf(this.native, info.os),
          osName: text(info.osName),
          osPrettyName: text(info.osPrettyName),
          image: text(info.image),
          imageDigest: text(info.imageDigest),
          kind: kindOf(this.native, info.kind),
          arch: text(info.arch),
          reachable: false,
          error: undefined,
          host: text(info.host),
          hostName: text(info.hostName),
          power: text(info.power),
          powerState: text(info.powerState),
          cloud: text(info.cloud),
          cloudPlace: text(info.cloudPlace),
          cloudDelete: text(info.cloudDelete),
        };
        // Turned off on purpose: no probe (it would only time out).
        if (info.powerState === "suspended" || info.powerState === "stopped") return row;
        // Bounded connect: an unreachable Space must not stall the list.
        const probe = await withTimeout(4, () => spaces.space(info.id));
        if (!probe.ok) {
          row.error = words(probe.error);
          return row;
        }
        row.reachable = true;
        const live = probe.value.info();
        row.os = osOf(this.native, live.os);
        row.osName = text(live.osName);
        row.osPrettyName = text(live.osPrettyName);
        if (live.image) row.image = live.image;
        if (live.imageDigest) row.imageDigest = live.imageDigest;
        row.kind = kindOf(this.native, live.kind) ?? row.kind;
        if (live.arch) row.arch = live.arch;
        let caps: string | null = null;
        try {
          caps = probe.value.capabilitiesJson();
        } catch {
          caps = null;
        }
        // What it answered with just now: a machine that does not share
        // its desktop reports the desktop's features unsupported.
        const features = supportedFeatures(caps);
        if (features) row.features = features;
        const hostname = capabilitiesHostname(caps);
        if (hostname) this.hostnames.set(info.id, hostname);
        return row;
      }),
    );
  }

  reportedHostname(id: string): string | null {
    return this.hostnames.get(id) ?? null;
  }

  async create(args: AppCreateSpaceArgs, createId: string, progress: (p: SpaceCreateProgress) => void): Promise<string> {
    const result = await this.cua.spaces().createWithProgress(
      {
        image: args.image,
        on: args.on,
        kind: args.kind,
        runtime: args.runtime,
        name: args.name,
        cpus: args.cpus,
        memoryMb: args.memoryMb === undefined ? undefined : BigInt(args.memoryMb),
        diskGb: args.diskGb,
        timeoutMs: undefined,
        wait: undefined,
        reuse: false,
        command: undefined,
        env: new Map(),
        services: new Map(),
        spacesd: args.spacesd,
        gpu: args.gpu,
        createId,
      },
      { onProgress: progress },
    );
    return result.space?.id ?? result.pendingId ?? "";
  }

  async cancelCreate(createId: string): Promise<void> {
    await this.cua.spaces().cancelCreate(createId);
  }

  async gpuChoices(): Promise<AppGpuChoice[] | null> {
    const support = await bounded(PROBE_SECONDS, () => this.cua.spaces().gpuSupport("local"));
    return support ? gpuChoices(support) : null;
  }

  async hosts(): Promise<AppSpaceHost[]> {
    const hosts = await bounded(PROBE_SECONDS, () => this.cua.spaces().hosts());
    return (hosts ?? []).map((h) => this.native.appSpaceHost(h));
  }

  async add(url: string, token: string | null, name: string | null): Promise<void> {
    await this.cua.spaces().add(url, token ?? undefined, name ?? undefined);
  }

  async remove(id: string, removeOnly: boolean): Promise<void> {
    if (removeOnly) await this.cua.spaces().remove(id);
    else await this.cua.spaces().delete_(id);
  }

  async setPower(id: string, on: boolean): Promise<void> {
    if (on) await this.cua.spaces().start(id);
    else await this.cua.spaces().stop(id);
  }

  async localRuntimes(): Promise<LocalRuntimes | null> {
    const report = await bounded(PROBE_SECONDS, () => this.cua.local().doctor());
    if (!report) return null;
    const ok = this.native.RuntimeCheckStatus.Ok;
    // Lume and the built-in Linux runtime count as ready when cua sets them
    // up by itself on the first create that needs them.
    const setsUpItself = new Set(["lume", "managed"]);
    const ready = report.checks
      .filter((c) => c.status === ok || (c.status === this.native.RuntimeCheckStatus.Installable && setsUpItself.has(c.name.toLowerCase())))
      .map((c) => c.name.toLowerCase());
    const details = new Map<string, string>();
    for (const c of report.checks) if (c.status !== ok) details.set(c.name.toLowerCase(), c.detail);
    return { ready, details };
  }

  private config(key: string): string | null {
    try {
      return this.native.configGet(key).value;
    } catch {
      return null;
    }
  }

  /** Lume (macOS VMs) runs on a Mac only: elsewhere there is no setting, so Settings has no macOS VMs row. */
  async lumeSource(): Promise<string | null> {
    return this.platform === "darwin" ? this.config("runtime.lume") : null;
  }

  async setLumeSource(value: string): Promise<void> {
    this.native.configSet("runtime.lume", value);
  }

  async linuxSource(): Promise<string | null> {
    return this.config("runtime.linux");
  }

  async setLinuxSource(value: string): Promise<void> {
    this.native.configSet("runtime.linux", value);
  }

  async localStorage(): Promise<LocalStorage | null> {
    return bounded(PROBE_SECONDS, () => this.cua.local().storage());
  }

  async cloudAvailable(): Promise<boolean> {
    if (this.env.FLEETS_TOKEN !== undefined || this.env.CUA_CLIENT_ID !== undefined) return true;
    return (await bounded(PROBE_SECONDS, () => this.cua.auth().status().loggedIn)) ?? false;
  }

  /** From `lume serve` (`$LUME_API`, else its default address); Lume is macOS only. */
  async runningMacosVms(): Promise<number | null> {
    if (process.platform !== "darwin") return null;
    const base = (this.env.LUME_API ?? "http://127.0.0.1:7777").replace(/\/+$/, "");
    const answer = await bounded(PROBE_SECONDS, async () => {
      const r = await this.fetchImpl(`${base}/lume/vms`, { signal: AbortSignal.timeout(PROBE_SECONDS * 1000) });
      return r.ok ? ((await r.json()) as unknown) : null;
    });
    return runningMacosVms(answer);
  }

  async cloudPricing(): Promise<AppCloudPricing | null> {
    if (!(await this.cloudAvailable())) return null;
    const p = await bounded(PROBE_SECONDS, () => this.cua.fleet().usagePricing());
    return p ? { vcpuHourUsd: p.vcpuHourUsd, memoryGibHourUsd: p.memoryGibHourUsd } : null;
  }

  async usage(id: string): Promise<AppSpaceUsage | null> {
    const u = await bounded(5, async () => (await this.cua.spaces().space(id)).usage());
    return u ? { memoryUsed: u.memoryUsed, memoryTotal: u.memoryTotal, memoryLimited: u.memoryLimited, diskUsed: u.diskUsed, diskTotal: u.diskTotal, diskLimited: u.diskLimited } : null;
  }

  async agentRuns(id: string): Promise<AppSpaceAgentRun[]> {
    const runs = await (await this.cua.spaces().space(id)).agentList();
    return this.native.appAgentRows(runs.map((r) => r.json));
  }

  async windows(id: string): Promise<StreamWindow[]> {
    return streamWindows(await (await this.cua.spaces().space(id)).windows(undefined));
  }

  async primaryDisplay(id: string): Promise<AppStreamDisplay | null> {
    const displays = await bounded(5, async () => (await this.cua.spaces().space(id)).displays());
    const d = displays?.[0];
    return d && d.widthPx > 0 && d.heightPx > 0 ? { widthPx: d.widthPx, heightPx: d.heightPx } : null;
  }

  async thumbnail(id: string, maxAgeMs: number | null): Promise<SpaceThumbnailData | null> {
    const t = await bounded(5, async () => (await this.cua.spaces().space(id)).thumbnail(maxAgeMs === null ? undefined : BigInt(Math.max(0, Math.round(maxAgeMs)))));
    if (!t || t.image.byteLength === 0) return null;
    return {
      image: new Uint8Array(t.image),
      format: t.format === this.native.ImageFormat.Png ? "png" : "jpeg",
      width: t.width,
      height: t.height,
      capturedAtMs: Number(t.capturedAtMs),
    };
  }

  async sendFiles(id: string, paths: string[]): Promise<AppSentFileInfo[]> {
    const space = await this.cua.spaces().space(id);
    const sent: AppSentFileInfo[] = [];
    for (const path of paths) {
      const report = await space.sendFile(path, { targetDirectory: undefined, respectIgnoreFiles: true, conflict: undefined });
      sent.push(this.native.appSentFile(report));
    }
    return sent;
  }

  async shares(id: string): Promise<AppShareEntryInput[]> {
    return shareRows(await (await this.cua.spaces().space(id)).shares());
  }

  async share(id: string, who: string, role: string): Promise<AppShareEntryInput[]> {
    return shareRows(await (await this.cua.spaces().space(id)).share(who, role));
  }

  async unshare(id: string, who: string): Promise<AppShareEntryInput[]> {
    return shareRows(await (await this.cua.spaces().space(id)).unshare(who));
  }

  async tool(tool: string, args: Record<string, unknown>): Promise<unknown> {
    const r = await this.cua.spaces().callToolJson(tool, JSON.stringify(args));
    if (r.isError) throw new ToolError(r.text.startsWith("error: ") ? r.text.slice(7) : r.text);
    try {
      return JSON.parse(r.text) as unknown;
    } catch {
      return r.text;
    }
  }

  async teleportContext(id: string): Promise<{ teleport: TeleportLike; space: SpaceLike }> {
    return { teleport: this.teleportHandle(), space: await this.cua.spaces().space(id) };
  }

  teleportHandle(): TeleportLike {
    this.teleport ??= this.native.teleport(this.cua);
    return this.teleport;
  }

  async appIcons(id: string, requests: SpaceAppIconRequest[]): Promise<(Uint8Array | null)[]> {
    try {
      const icons = await (await this.cua.spaces().space(id)).appIcons(requests);
      return icons.map((i) => (i ? new Uint8Array(i.bytes) : null));
    } catch {
      return requests.map(() => null);
    }
  }
}
