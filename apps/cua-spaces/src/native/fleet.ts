// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { SpaceRow } from "../model/spaces";
import type { Location, Runtime, SpaceKind } from "../model/types";
import { hasTauri } from "./bridge";
import type { LocalStatus } from "./local";

/**
 * Typed wrapper over the shell's Spaces commands. The shell is a thin layer over the cua
 * SDK (`cua-spaces` + `cua daemon`): every location (Cua Cloud, this Mac,
 * Spaces added by address) comes back through one registry.
 *
 * Outside Tauri (browser dev, Vitest) the fallback reports "not native" so the
 * portal keeps its synthetic fixtures and no network is ever touched.
 */

/**
 * What starting a sign-in returns. `browser`: finish in the tab that just
 * opened. `device`: enter `userCode` at `verificationUri`.
 */
export interface SignInStart {
  method?: "browser" | "device";
  userCode?: string;
  verificationUri: string;
}

/** Cua Cloud account status. */
export interface FleetStatus {
  configured: boolean;
  authMode: "user" | "client-credentials" | "static-token" | "none";
  baseUrl: string;
  tokenUrl: string;
  /** OAuth client id, when configured; shown muted in Settings. */
  clientId?: string;
  /** Signed-in user identity (email/subject), when a user session is active. */
  identity?: string;
  namespaces?: string[];
  probeError?: string;
}

/** The local `cua daemon` the app shares Spaces with (CLI / MCP clients). */
export interface DaemonStatus {
  connected: boolean;
  version?: string;
  socketPath?: string;
  loopbackUrl?: string;
  error?: string;
}

/** What the viewer page needs to know about its Space. */
export interface ViewerWindowRequest {
  id: string;
  name: string;
  /** Who is driving the Space (PiP badge); informational only. */
  controller?: "agent" | "you";
  os?: string;
}

export interface ViewerConfig {
  view: "space" | "pip" | "windows";
  space: ViewerWindowRequest;
  /** Most recent cached screenshot (`data:` URL) for the blurred connecting
   * background (item D); absent when nothing is cached yet. */
  lastScreenshot?: string;
  /** An in-flight teleport transfer overlaying this Space window (item B). */
  transfer?: {
    phase: "active" | "error";
    appName: string;
    message?: string;
    sentBytes?: number;
    totalBytes?: number;
  };
  /** The single remote window this stream window is dedicated to (the
   * `winone-*` per-window path). When present the viewer runs in SINGLE mode.
   * `mediaUrl` (MCP-driven `stream_space_window`) is a ready media ticket URL
   * to attach to instead of opening a session itself. */
  targetWindow?: {
    id: string;
    appName: string;
    title: string;
    replica?: boolean;
    mediaUrl?: string;
    mediaSessionId?: string;
  };
}

/**
 * `create_space` options, the SDK's placement model: where (`on`), what
 * (`kind`) and which engine (`runtime`). Omitted, `on` is the default
 * location (Settings, `cua config set default.on`), `kind` and `runtime`
 * are `auto`. The SDK validates the combination; its error lists the valid
 * values and is user-facing.
 */
export interface SpaceCreateConfig {
  image?: string;
  /** `local`, `cloud`, or a connected cloud's word (`aws`, `gcp`, `modal`). */
  on?: Location | string;
  kind?: SpaceKind | "auto";
  runtime?: Runtime;
  name?: string;
  /** This Mac only. */
  cpus?: number;
  /** This Mac only. */
  memoryMb?: number;
  /** This Mac's VMs only: grow the disk to this many GB. */
  diskGb?: number;
  /** Whether the image runs cua-spacesd; omitted, the SDK decides. */
  spacesd?: boolean;
  /** Return a reachable registered Space in that location instead. */
  reuse?: boolean;
  /** A GPU option of the runtime (`paravirtual`, from `gpuSupport`). */
  gpu?: string;
}

/** Where new Spaces go by default (`default.on`), and where that came from. */
export interface DefaultLocation {
  /** `local`, `cloud`, or a connected cloud's word (`aws`). */
  value: Location | string;
  /** `env` (CUA_DEFAULT_ON, read-only here), `config` or `default`. */
  source: "env" | "config" | "default";
  /** The environment variable, when `source` is `env`. */
  env?: string;
  /** The config file (`$CUA_HOME/config.toml`). */
  path: string;
}

/** Used off the native shell, and when the shell cannot read the setting. */
export const BUILTIN_DEFAULT_LOCATION: DefaultLocation = {
  value: "local",
  source: "default",
  path: "~/.cua/config.toml",
};

export type StreamTarget = { kind: "display"; displayId?: string } | { kind: "window"; windowId: string };

export interface StreamOpts {
  maxFps?: number;
  maxDimension?: number;
  audio?: boolean;
  codecs?: string[];
  /** Input policy. The shell defaults to `allow_activation`; the view-only
   * PiP passes `view_only`. Without an input-capable policy the driver
   * refuses InteractiveInput. */
  policy?: "view_only" | "background_only" | "allow_activation";
  /** Single-window mode that resizes the remote window (geometry control). */
  geometryControl?: boolean;
}

/** A minted media ticket (rcdp wire v2). The ticket is scoped to one session
 * and safe in a URL. */
export interface StreamTicketInfo {
  spaceId: string;
  mediaSessionId: string;
  wsUrl: string;
  ticket: string;
  ticketExpiresAt?: string;
  codec: "h264" | "bgra" | "png";
  wireVersion: number;
  frameSize: [number, number];
  via: "direct" | "daemon";
  audio: boolean;
}

/** Whether this Mac is currently sharing its network to a Space, and which. */
export interface HotspotStatus {
  active: boolean;
  spaceId: string | null;
}

/** `spaces:create-progress`: what a create is doing. */
export interface CreateProgress {
  pendingId: string;
  /** `preparing`, `pulling`, `creating`, `booting`, `waiting_for_services`,
   * `connecting` or `ready`. */
  phase: string;
  /** 0 to 1 through the phase, when its size is known. */
  fraction?: number | null;
  detail: string;
  /** An image download's bytes so far, of how many, and how fast
   * (smoothed), when the step counts them. */
  bytesDone?: number | null;
  bytesTotal?: number | null;
  bytesPerSecond?: number | null;
  /** The id the Space will have (`local:<name>`), when known. */
  space?: string | null;
}

/** `cancel_create`: what a cancel found and did (the SDK's `CancelOutcome`). */
/** What turning a Space off or on did (the SDK's `SpacePower`). */
export interface SpacePowerReport {
  space: string;
  /** `running`, `suspended` or `stopped`: the state it is in now. */
  state: string;
  /** How it turns off: `suspend` or `stop`. */
  power: string;
  /** For people. */
  message: string;
}

export interface CancelOutcome {
  /** The Space id the create had, when known. */
  id: string;
  state: "cancelled" | "not_creating" | "already_created";
  /** For people: what was removed and what stays. */
  message: string;
}

/** A runtime's GPU option, as the New Space wizard offers it (the app
 * core's `GpuChoice`). */
export interface GpuChoice {
  runtime: string;
  id: string;
  label: string;
  experimental: boolean;
  supported: boolean;
  reason?: string | null;
  learnMore?: string | null;
}

/** This account's Cua Cloud rates (the app core's `CloudPricing`). */
export interface CloudPricing {
  /** USD per reserved vCPU per hour. */
  vcpuHourUsd: number;
  /** USD per reserved GB of memory per hour. */
  memoryGibHourUsd: number;
}

/** The account's Cua Cloud billing (the app core's `billing::BillingStatus`):
 * the credit left and the website billing page. The app takes no payment
 * details; the website does. */
export interface BillingStatus {
  billingEnabled: boolean;
  card: { brand: string; last4: string } | null;
  credit: { balanceUsdCents: number } | null;
  billingUrl: string | null;
}

export interface FleetBridge {
  readonly isNative: boolean;
  status(probe?: boolean): Promise<FleetStatus>;
  /** This account's Cua Cloud rates; null when unknown (no estimate). */
  cloudPricing?(): Promise<CloudPricing | null>;
  /** The account's Cua Cloud billing (null: no Fleet client). */
  billingStatus?(): Promise<BillingStatus | null>;
  daemonStatus(): Promise<DaemonStatus>;
  ensureDaemon(): Promise<DaemonStatus>;
  localStatus(): Promise<LocalStatus>;
  /** The SDK registry, every provider. */
  listSpaces(): Promise<SpaceRow[]>;
  /** One registered Space, connected (cheaper than `listSpaces` for a viewer). */
  spaceInfo?(spaceId: string): Promise<SpaceRow>;
  /** "Add Space by address": `host:port` or `http(s)://host:port`. */
  addSpace(url: string, token?: string, name?: string): Promise<SpaceRow>;
  /** Creates a Space (in the default location unless `config.on` says);
   * resolves once its spacesd answers. With `pendingId`, its progress
   * arrives on `onCreateProgress` under that id. */
  createSpace(config?: SpaceCreateConfig, pendingId?: string): Promise<SpaceRow>;
  /** Cancels the create started with `pendingId`: it stops and what it
   * made is removed; resolves once that is done. The create then rejects
   * with an error starting `cancelled: `. */
  cancelCreate?(pendingId: string): Promise<CancelOutcome>;
  /** The GPU option of each local runtime that has one (the wizard's
   * GPU row). */
  gpuSupport?(): Promise<GpuChoice[]>;
  /** Your machines that provide Spaces (the wizard's Run on menu). */
  listHosts?(): Promise<import("../components/desktop/NewSpaceWizard").SpaceHost[]>;
  /** This Mac's CPU architecture (`aarch64`, `x86_64`). */
  hostArch?(): Promise<string>;
  /** The SDK's create progress (pulling, booting, waiting for cua-spacesd,
   * connecting) of creates started with a pending id. */
  onCreateProgress?(handler: (progress: CreateProgress) => void): Promise<() => void>;
  /** Deletes a created Space's sandbox; a Space added by address is only
   * forgotten. */
  deleteSpace(spaceId: string): Promise<string>;
  /** Turns a Space off (`on` false: suspended or stopped, as its provider
   * can) or back on: the power button next to Delete. */
  setSpacePower?(spaceId: string, on: boolean): Promise<SpacePowerReport>;
  /** Forget only (the machine is untouched). */
  removeSpace(spaceId: string): Promise<void>;
  /** Where new Spaces go by default. */
  defaultLocation(): Promise<DefaultLocation>;
  /** Stores the default location in `$CUA_HOME/config.toml`. */
  setDefaultLocation(on: Location): Promise<DefaultLocation>;
  /** Extend a cloud Space's lease. */
  keepAliveSpace(spaceId: string, seconds: number): Promise<void>;
  /** Fires after add/create/delete/remove anywhere in the app. */
  onSpacesChanged(handler: () => void): Promise<() => void>;
  /** Returns a `data:image/…;base64,…` URL. */
  screenshot(spaceId: string, maxDimension?: number): Promise<string>;
  openStream(spaceId: string, target: StreamTarget, options?: StreamOpts): Promise<StreamTicketInfo>;
  closeStream(spaceId: string, mediaSessionId: string): Promise<void>;
  /**
   * Start sharing THIS Mac's network with `spaceId` (the Mac becomes the
   * egress "hotspot"). Replaces any previous hotspot.
   */
  startHotspot(spaceId: string): Promise<HotspotStatus>;
  /** Stop sharing the network. Idempotent. */
  stopHotspot(): Promise<HotspotStatus>;
  /** Current hotspot state (for reconciling the notch indicator on launch). */
  hotspotStatus(): Promise<HotspotStatus>;
  /** Fires whenever the hotspot turns on/off (including if the tunnel drops). */
  onHotspotChanged(handler: (status: HotspotStatus) => void): Promise<() => void>;
  viewerConfig(): Promise<ViewerConfig>;
  openSpaceWindow(space: ViewerWindowRequest): Promise<void>;
  /** Open (or re-target) the teleport picker window for this Space. */
  openTeleportPicker(space: ViewerWindowRequest): Promise<void>;
  /** Open the teleport picker PRE-TARGETED to a coding-agent provider. */
  launchAgent(space: ViewerWindowRequest, appId: string, appName: string): Promise<void>;
  /**
   * Install a coding agent (cua-agents harness id) in the Space and open its
   * interactive CLI in a terminal on the Space's desktop. Resolves once the
   * terminal is launched; the user signs in inside it.
   */
  launchAgentTerminal(space: ViewerWindowRequest, harness: string): Promise<void>;
  pinSpacePip(space: ViewerWindowRequest): Promise<void>;
  unpinSpacePip(spaceId: string): Promise<void>;
  /** Size the PiP window to the remote desktop's aspect ratio and lock it. */
  setPipAspect(spaceId: string, width: number, height: number): Promise<void>;
  /** Toggle per-window streaming (each remote window as its own native window). */
  setWindowStream(space: ViewerWindowRequest, enabled: boolean): Promise<void>;
  /** Resize the CURRENT stream window's inner area (single mode). */
  resizeStreamWindow(width: number, height: number): Promise<void>;
  /** Open a URL in the user's default browser. */
  openExternal(url: string): Promise<void>;
  /**
   * "Sign in to Cua" through the SDK: browser sign-in (PKCE, loopback
   * redirect), else a device code. The session is shared with the cua CLI.
   */
  beginSignIn(): Promise<SignInStart>;
  /** Clear the signed-in user session; env credentials resume as fallback. */
  signOut(): Promise<void>;
  onSignedIn(handler: (identity?: string) => void): Promise<() => void>;
  onSignInFailed(handler: (reason: string) => void): Promise<() => void>;
  onSignedOut(handler: () => void): Promise<() => void>;
}

const NOT_NATIVE = "Spaces commands need the Tauri shell";

export function createFallbackFleetBridge(): FleetBridge {
  const unavailable = () => Promise.reject(new Error(NOT_NATIVE));
  const noopUnsub = async () => () => {};
  return {
    isNative: false,
    status: async () => ({
      configured: false,
      authMode: "none",
      baseUrl: "",
      tokenUrl: "",
    }),
    daemonStatus: async () => ({ connected: false, error: "not running in the app" }),
    ensureDaemon: unavailable,
    localStatus: async () => ({
      available: false,
      backends: [],
      containerImage: "",
      macosImage: null,
      error: "not running in the app",
    }),
    listSpaces: async () => [],
    addSpace: unavailable,
    createSpace: unavailable,
    deleteSpace: unavailable,
    removeSpace: unavailable,
    defaultLocation: async () => BUILTIN_DEFAULT_LOCATION,
    setDefaultLocation: unavailable,
    keepAliveSpace: unavailable,
    onSpacesChanged: noopUnsub,
    screenshot: unavailable,
    openStream: unavailable,
    closeStream: async () => {},
    startHotspot: unavailable,
    stopHotspot: async () => ({ active: false, spaceId: null }),
    hotspotStatus: async () => ({ active: false, spaceId: null }),
    onHotspotChanged: noopUnsub,
    viewerConfig: unavailable,
    openSpaceWindow: async () => {},
    openTeleportPicker: async () => {},
    launchAgent: async () => {},
    launchAgentTerminal: unavailable,
    pinSpacePip: async () => {},
    unpinSpacePip: async () => {},
    setPipAspect: async () => {},
    setWindowStream: async () => {},
    resizeStreamWindow: async () => {},
    openExternal: async () => {},
    beginSignIn: unavailable,
    signOut: async () => {},
    onSignedIn: noopUnsub,
    onSignInFailed: noopUnsub,
    onSignedOut: noopUnsub,
  };
}

export function createTauriFleetBridge(): FleetBridge {
  // Imported lazily so the browser/test bundle never touches Tauri globals.
  const core = import("@tauri-apps/api/core");
  const event = import("@tauri-apps/api/event");
  const invoke = async <T>(command: string, args?: Record<string, unknown>) =>
    (await core).invoke<T>(command, args);
  const on = async <T>(name: string, handler: (payload: T) => void) => {
    const { listen } = await event;
    return listen<T>(name, (e) => handler(e.payload));
  };
  return {
    isNative: true,
    status: (probe) => invoke<FleetStatus>("fleet_status", { probe: probe ?? false }),
    daemonStatus: () => invoke<DaemonStatus>("daemon_status"),
    ensureDaemon: () => invoke<DaemonStatus>("ensure_daemon"),
    localStatus: () => invoke<LocalStatus>("local_status"),
    cloudPricing: () => invoke<CloudPricing | null>("cloud_pricing"),
    billingStatus: () => invoke<BillingStatus | null>("billing_status"),
    listSpaces: () => invoke<SpaceRow[]>("list_spaces"),
    spaceInfo: (spaceId) => invoke<SpaceRow>("space_info", { spaceId }),
    addSpace: (url, token, name) =>
      invoke<SpaceRow>("add_space", { url, token: token || null, name: name || null }),
    createSpace: (config, pendingId) =>
      invoke<SpaceRow>("create_space", { config: config ?? null, pendingId: pendingId ?? null }),
    onCreateProgress: (handler) => on<CreateProgress>("spaces:create-progress", handler),
    cancelCreate: (pendingId) => invoke<CancelOutcome>("cancel_create", { pendingId }),
    gpuSupport: () => invoke<GpuChoice[]>("gpu_support"),
    listHosts: () => invoke<import("../components/desktop/NewSpaceWizard").SpaceHost[]>("list_hosts"),
    hostArch: () => invoke<string>("host_arch"),
    deleteSpace: (spaceId) => invoke<string>("delete_space", { spaceId }),
    setSpacePower: (spaceId, on) => invoke<SpacePowerReport>("set_space_power", { spaceId, on }),
    removeSpace: (spaceId) => invoke<void>("remove_space", { spaceId }),
    defaultLocation: () => invoke<DefaultLocation>("get_default_location"),
    setDefaultLocation: (on) => invoke<DefaultLocation>("set_default_location", { on }),
    keepAliveSpace: (spaceId, seconds) => invoke<void>("keep_alive_space", { spaceId, seconds }),
    onSpacesChanged: (handler) => on<unknown>("spaces:changed", () => handler()),
    screenshot: (spaceId, maxDimension) =>
      invoke<string>("space_screenshot", { spaceId, maxDimension: maxDimension ?? null }),
    openStream: (spaceId, target, options) =>
      invoke<StreamTicketInfo>("open_space_stream", { spaceId, target, options: options ?? null }),
    closeStream: (spaceId, mediaSessionId) =>
      invoke<void>("close_space_stream", { spaceId, mediaSessionId }),
    startHotspot: (spaceId) => invoke<HotspotStatus>("start_hotspot", { spaceId }),
    stopHotspot: () => invoke<HotspotStatus>("stop_hotspot"),
    hotspotStatus: () => invoke<HotspotStatus>("hotspot_status"),
    onHotspotChanged: (handler) =>
      on<HotspotStatus>("hotspot:changed", (p) => handler(p ?? { active: false, spaceId: null })),
    viewerConfig: () => invoke<ViewerConfig>("viewer_config"),
    openSpaceWindow: (space) => invoke<void>("open_space_window", { space }),
    // `TeleportPickerRequest` is camelCase: `spaceId`/`spaceName`/`app`.
    openTeleportPicker: (space) =>
      invoke<void>("open_teleport_picker", {
        request: { spaceId: space.id, spaceName: space.name },
      }),
    launchAgent: (space, appId, appName) =>
      invoke<void>("open_teleport_picker", {
        request: {
          spaceId: space.id,
          spaceName: space.name,
          app: { id: appId, name: appName },
        },
      }),
    launchAgentTerminal: (space, harness) =>
      invoke<void>("launch_agent_terminal", { spaceId: space.id, harness }),
    pinSpacePip: (space) => invoke<void>("pin_space_pip", { space }),
    unpinSpacePip: (spaceId) => invoke<void>("unpin_space_pip", { spaceId }),
    setPipAspect: (spaceId, width, height) =>
      invoke<void>("set_pip_aspect", { spaceId, width, height }),
    setWindowStream: (space, enabled) => invoke<void>("set_window_stream", { space, enabled }),
    resizeStreamWindow: (width, height) =>
      invoke<void>("resize_stream_window", { width, height }),
    openExternal: (url) => invoke<void>("open_external", { url }),
    beginSignIn: () => invoke<SignInStart>("begin_sign_in"),
    signOut: () => invoke<void>("sign_out"),
    onSignedIn: (handler) =>
      on<{ identity?: string } | null>("auth:signed-in", (p) => handler(p?.identity)),
    onSignInFailed: (handler) =>
      on<{ reason?: string } | null>("auth:sign-in-failed", (p) =>
        handler(p?.reason ?? "Sign-in failed"),
      ),
    onSignedOut: (handler) => on<unknown>("auth:signed-out", () => handler()),
  };
}

export function createFleetBridge(): FleetBridge {
  return hasTauri() ? createTauriFleetBridge() : createFallbackFleetBridge();
}
