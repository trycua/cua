// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { hasTauri } from "./bridge";

/**
 * "This machine" as a host: the shell's `cua-host` commands. Host setup installs cua-spacesd as a
 * per-OS service that joins the cua.ai relay (default) or listens on a direct
 * `ip:port` (Advanced). Outside Tauri the fallback reports "not configured"
 * and treats onboarding as done, so browser dev and tests never see the
 * first-run screen unless they inject a bridge.
 */

export type HostModeName = "relay" | "direct";
export type OnboardingMode = "client" | "host";

export interface ServiceState {
  installed: boolean;
  running: boolean;
  kind: "systemd" | "launchd" | "windows-task" | "process" | string;
  detail?: string;
}

/** Someone connected to this machine right now (relay presence). */
export interface ConnectedClient {
  id: string;
  email?: string;
  name?: string;
  streams?: number;
  since?: number;
}

/** An OS permission the host needs and the user must grant themselves. */
export interface PermissionHint {
  /** e.g. "screen-recording", "accessibility". */
  id: string;
  label: string;
  /** What to turn on there. */
  instructions?: string;
  /** Settings pane URL (opened only when the user clicks). */
  settingsUrl?: string;
  granted?: boolean;
}

export interface HostStatus {
  configured: boolean;
  mode?: HostModeName | null;
  relayUrl?: string | null;
  directUrl?: string | null;
  machineId?: string | null;
  name?: string | null;
  sharing: boolean;
  service: ServiceState;
  online?: boolean | null;
  clients: ConnectedClient[];
  permissions: PermissionHint[];
  error?: string | null;
  /** Who reached this machine recently, newest first (the access log). */
  recentAccess?: AccessRecord[];
  /** Set when the access log does not verify. */
  accessLogError?: string | null;
  /** This machine's own desktop is a Space (default on). */
  shareDesktop?: boolean;
  /** This machine creates Spaces for your other devices. */
  provideSpaces?: boolean;
  /** Provided Spaces at once (0: no limit). */
  maxSpaces?: number;
  /** macOS VMs at once (at most two). */
  maxMacosVms?: number;
  /** The Spaces this machine provides now. */
  providedSpaces?: ProvidedSpace[];
  /** Remote creates, deletes and refusals, and settings changes, newest first. */
  spacesAudit?: SpacesAuditRecord[];
  /** Set when the Spaces audit does not verify. */
  spacesAuditError?: string | null;
}

/** A Space this machine provides to one of your devices. */
export interface ProvidedSpace {
  relayMachine: string;
  localSpace: string;
  name: string;
  image: string;
  os: string;
  kind: string;
  createdBy: string;
  createdAtMs: number;
}

/** One line of the Spaces audit. */
export interface SpacesAuditRecord {
  atMs: number;
  action: string;
  who: string;
  space: string;
  detail: string;
}

/** A change to this machine's two settings (the core's `host.settingChange`). */
export interface HostSettingChange {
  shareDesktop?: boolean | null;
  provideSpaces?: boolean | null;
}

export interface AccessRecord {
  atMs: number;
  via: string;
  who: string;
  what: string;
}

export interface HostSetupRequest {
  mode: HostModeName;
  /** Relay URL; the shell defaults to https://relay.cua.ai. */
  relayUrl?: string;
  /** Direct `ip:port` (Advanced). */
  direct?: string;
  name?: string;
  allow?: string[];
  /** `desktop` (share this desktop) or `spare` (only run Spaces). */
  profile?: "desktop" | "spare" | null;
  shareDesktop?: boolean | null;
  provideSpaces?: boolean | null;
}

export interface OnboardingState {
  completed: boolean;
  mode?: OnboardingMode | null;
  /** Mode preselected by the installer (`--mode host|client` or MDM file). */
  installerMode?: OnboardingMode | null;
}

export interface HostBridge {
  readonly isNative: boolean;
  status(): Promise<HostStatus>;
  setup(request: HostSetupRequest): Promise<HostStatus>;
  stopSharing(): Promise<HostStatus>;
  startSharing(): Promise<HostStatus>;
  remove(): Promise<void>;
  /** Change what this machine shares (its desktop, Spaces for your devices). */
  configure(change: HostSettingChange): Promise<HostStatus>;
  onboardingState(): Promise<OnboardingState>;
  completeOnboarding(mode: OnboardingMode): Promise<void>;
  /** Open an OS settings pane (permission hints); only on a user click. */
  openSettings(url: string): Promise<void>;
}

export const DEFAULT_RELAY_URL = "https://relay.cua.ai";

export function unconfiguredStatus(error?: string): HostStatus {
  return {
    configured: false,
    sharing: false,
    service: { installed: false, running: false, kind: "process" },
    clients: [],
    permissions: [],
    error: error ?? null,
  };
}

export function createFallbackHostBridge(): HostBridge {
  const unavailable = () => Promise.reject(new Error("Host setup needs the Cua Spaces app"));
  return {
    isNative: false,
    status: async () => unconfiguredStatus(),
    setup: unavailable,
    stopSharing: unavailable,
    startSharing: unavailable,
    remove: unavailable,
    configure: unavailable,
    onboardingState: async () => ({ completed: true }),
    completeOnboarding: async () => {},
    openSettings: async () => {},
  };
}

export function createTauriHostBridge(): HostBridge {
  const core = import("@tauri-apps/api/core");
  const invoke = async <T>(command: string, args?: Record<string, unknown>) =>
    (await core).invoke<T>(command, args);
  return {
    isNative: true,
    status: () => invoke<HostStatus>("host_status"),
    setup: (request) => invoke<HostStatus>("host_setup", { request }),
    stopSharing: () => invoke<HostStatus>("host_stop_sharing"),
    startSharing: () => invoke<HostStatus>("host_start_sharing"),
    remove: () => invoke<void>("host_remove"),
    configure: (change) => invoke<HostStatus>("host_configure", { change }),
    onboardingState: () => invoke<OnboardingState>("onboarding_state"),
    completeOnboarding: (mode) => invoke<void>("complete_onboarding", { mode }),
    openSettings: (url) => invoke<void>("host_open_settings", { url }),
  };
}

export function createHostBridge(): HostBridge {
  return hasTauri() ? createTauriHostBridge() : createFallbackHostBridge();
}

/**
 * In-memory host for tests and design work: setup succeeds (or fails with
 * `failSetup`), sharing toggles, clients can be injected.
 */
export function createFakeHostBridge(
  options: {
    onboarding?: OnboardingState;
    status?: HostStatus;
    failSetup?: string;
    platform?: "macos" | "linux" | "windows";
  } = {},
): HostBridge & { calls: string[]; state: { status: HostStatus; onboarding: OnboardingState } } {
  const state = {
    status: options.status ?? unconfiguredStatus(),
    onboarding: options.onboarding ?? { completed: false },
  };
  const calls: string[] = [];
  return {
    isNative: true,
    calls,
    state,
    status: async () => state.status,
    setup: async (request) => {
      calls.push(`setup:${request.mode}:${request.mode === "direct" ? request.direct : request.relayUrl ?? DEFAULT_RELAY_URL}`);
      if (options.failSetup) throw new Error(options.failSetup);
      const spare = request.profile === "spare";
      state.status = {
        shareDesktop: request.shareDesktop ?? !spare,
        provideSpaces: request.provideSpaces ?? spare,
        maxSpaces: 4,
        maxMacosVms: options.platform === "macos" ? 2 : 0,
        providedSpaces: [],
        spacesAudit: [],
        configured: true,
        mode: request.mode,
        relayUrl: request.mode === "relay" ? request.relayUrl ?? DEFAULT_RELAY_URL : null,
        directUrl: request.mode === "direct" ? `http://${request.direct}` : null,
        machineId: "0123abcd4567",
        name: request.name ?? "This machine",
        sharing: true,
        service: {
          installed: true,
          running: true,
          kind: options.platform === "macos" ? "launchd" : options.platform === "windows" ? "windows-task" : "systemd",
        },
        online: true,
        clients: [],
        permissions:
          options.platform === "macos"
            ? [
                {
                  id: "screen-recording",
                  label: "Screen Recording",
                  settingsUrl: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
                },
                {
                  id: "accessibility",
                  label: "Accessibility",
                  settingsUrl: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                },
              ]
            : [],
      };
      return state.status;
    },
    stopSharing: async () => {
      calls.push("stop");
      state.status = { ...state.status, sharing: false, clients: [] };
      return state.status;
    },
    startSharing: async () => {
      calls.push("start");
      state.status = { ...state.status, sharing: true };
      return state.status;
    },
    remove: async () => {
      calls.push("remove");
      state.status = unconfiguredStatus();
    },
    configure: async (change) => {
      calls.push(`configure:${JSON.stringify(change)}`);
      state.status = {
        ...state.status,
        shareDesktop: change.shareDesktop ?? state.status.shareDesktop,
        provideSpaces: change.provideSpaces ?? state.status.provideSpaces,
      };
      return state.status;
    },
    onboardingState: async () => state.onboarding,
    completeOnboarding: async (mode) => {
      calls.push(`onboarding:${mode}`);
      state.onboarding = { ...state.onboarding, completed: true, mode };
    },
    openSettings: async (url) => {
      calls.push(`settings:${url}`);
    },
  };
}
