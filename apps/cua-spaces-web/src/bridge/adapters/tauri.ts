// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Tauri shell (apps/cua-spaces/src-tauri): each operation is the same
 * `invoke` command the Tauri app's `src/native/*.ts` adapters call, with the
 * same argument names. No `@tauri-apps/api` dependency: Tauri 2 exposes
 * `invoke` on `window.__TAURI_INTERNALS__` (and on `__TAURI__.core` with
 * `app.withGlobalTauri`). Pushed events need `__TAURI__.event`; without it
 * the bridge polls the registry instead.
 *
 * Agents: persistent agents and pause/resume go through `agents_tool` (the
 * daemon's Spaces tools the Agents page may call), runs through
 * `list_space_agents`, and the coding agents on this machine through
 * `agent_setup_detect` / `agent_setup_configure`. A run's events
 * (`agent_events`) have no Tauri command yet, so `agents.events` is
 * unsupported here (`TAURI_UNSUPPORTED`).
 */

import { Emitter, UnsupportedOperationError, type DataAdapter } from "../adapter";
import type {
  CreateProgress,
  DaemonStatus,
  DefaultLocation,
  FleetStatus,
  HostStatus,
  LoginItemStatus,
  MachineRow,
  OnboardingState,
  SpaceHost,
  TelemetryView,
} from "../contracts/host";
import type { AgentSetupRow, PersistentAgent, SpaceAgentRun } from "../contracts/agents";
import type { HostWindow } from "../detect";
import { newSpaceTauriOps } from "../ops/new-space";
import { tauriShareOps } from "../ops/share";
import { tauriAgentKeysOps } from "../ops/agent-keys";
import { KEYVAULT_MANAGE_TAURI_UNSUPPORTED } from "../ops/keyvault-manage";
import { tauriKeyvaultSetupOps } from "../ops/keyvault-setup";
import { TELEPORT_TAURI_UNSUPPORTED, tauriTeleportOps } from "../ops/teleport";
import { tauriVolumeOps } from "../ops/volume";
import { tauriNotificationsOps } from "../ops/notifications";
import { TAURI_SETTINGS_UNSUPPORTED, tauriSettingsOps } from "../ops/settings";
import {
  DEFAULT_SETTINGS,
  UI_STORAGE_KEYS,
  type AppearanceTheme,
  type HostEvent,
  type OpArgs,
  type OpName,
  type OpResult,
  type SettingsSnapshot,
  loginItemOn,
} from "../protocol";
import { tauriHostSetupOps } from "../ops/host-setup";
import { TAURI_SPACE_DETAIL_UNSUPPORTED, tauriSpaceDetailOps } from "../ops/space-detail";
import { STREAM_OPERATIONS } from "../ops/stream";
import { tauriTelemetryOps } from "../ops/telemetry";
import { tauriStartupOps } from "../ops/startup";

type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

/** The Tauri event name for each pushed event (src-tauri emits these). */
export const TAURI_EVENTS = {
  "spaces:changed": (): HostEvent => ({ type: "spaces.changed" }),
  "spaces:create-progress": (p: unknown): HostEvent => ({ type: "spaces.createProgress", progress: p as CreateProgress }),
  "auth:signed-in": (p: unknown): HostEvent => ({
    type: "session.signedIn",
    identity: (p as { identity?: string } | null)?.identity,
  }),
  "auth:sign-in-failed": (p: unknown): HostEvent => ({
    type: "session.signInFailed",
    reason: (p as { reason?: string } | null)?.reason ?? "Sign-in failed",
  }),
  "auth:signed-out": (): HostEvent => ({ type: "session.signedOut" }),
} as const;

/** Operations the Tauri shell has no command for; they reject with `UnsupportedOperationError`. */
export const TAURI_UNSUPPORTED: readonly OpName[] = [
  "settings.choose",
  "agents.events",
  "agents.setupDriver",
  ...TELEPORT_TAURI_UNSUPPORTED,
  ...KEYVAULT_MANAGE_TAURI_UNSUPPORTED,
  ...TAURI_SETTINGS_UNSUPPORTED,
  ...TAURI_SPACE_DETAIL_UNSUPPORTED,
  ...STREAM_OPERATIONS,
];

/** `persistent_agent_list`'s snake_case agents, as `PersistentAgent`s (as `src/native/persistent.ts` maps them). */
export function persistentAgentsFromTool(answer: unknown): PersistentAgent[] {
  const agents = (answer as { agents?: unknown[] } | null)?.agents ?? [];
  return agents.map((raw) => {
    const a = raw as Record<string, unknown>;
    return {
      name: String(a.name ?? ""),
      harness: String(a.harness ?? ""),
      space: String(a.space ?? ""),
      paused: Boolean(a.paused),
      spaceState: typeof a.space_state === "string" ? a.space_state : "running",
      runId: typeof a.run_id === "string" ? a.run_id : null,
      savedMs: typeof a.saved_ms === "number" ? a.saved_ms : 0,
      lastError: typeof a.last_error === "string" ? a.last_error : null,
    };
  });
}

export function createTauriAdapter(win: HostWindow = globalThis.window as HostWindow): DataAdapter {
  const invokeFn: Invoke | undefined = win.__TAURI__?.core?.invoke ?? win.__TAURI_INTERNALS__?.invoke;
  if (!invokeFn) throw new Error("Tauri's invoke is not available");
  const invoke = <T>(cmd: string, args?: Record<string, unknown>) => invokeFn(cmd, args) as Promise<T>;
  const listen = win.__TAURI__?.event?.listen;
  const events = new Emitter();
  const unlisteners: Promise<() => void>[] = [];
  let listening = false;

  const startListening = () => {
    if (listening || !listen) return;
    listening = true;
    for (const [name, map] of Object.entries(TAURI_EVENTS)) {
      unlisteners.push(listen(name, (e) => events.emit(map(e.payload))));
    }
  };

  const storage = () => win.__CUA_UI_STORAGE__ ?? {};
  const readUi = (key: string) => storage()[key];
  const writeUi = async (key: string, value: string) => {
    if (win.__CUA_UI_STORAGE__) win.__CUA_UI_STORAGE__[key] = value;
    await invoke<void>("ui_storage_set", { key, value });
  };

  const readSettings = async (): Promise<SettingsSnapshot> => {
    const [telemetry, location, loginItem] = await Promise.all([
      invoke<TelemetryView>("telemetry_status").catch(() => null),
      invoke<DefaultLocation>("get_default_location").catch(() => null),
      invoke<LoginItemStatus>("login_item_status").catch(() => null),
    ]);
    const theme = readUi(UI_STORAGE_KEYS.theme);
    const menuBar = readUi(UI_STORAGE_KEYS.menuBar);
    return {
      values: {
        theme: theme === "light" || theme === "dark" || theme === "system" ? (theme as AppearanceTheme) : DEFAULT_SETTINGS.theme,
        menuBar: menuBar === undefined ? DEFAULT_SETTINGS.menuBar : menuBar === "true",
        hotkey: readUi(UI_STORAGE_KEYS.hotkey) ?? DEFAULT_SETTINGS.hotkey,
        telemetry: telemetry?.enabled ?? DEFAULT_SETTINGS.telemetry,
        defaultLocation: location?.value ?? DEFAULT_SETTINGS.defaultLocation,
        launchAtLogin: loginItemOn(loginItem),
        // The Tauri updater has one feed; there is no channel to pick.
        updateChannel: null,
      },
      defaultLocation: location,
      telemetry,
    };
  };

  const ops: { [K in OpName]?: (args: OpArgs<K>) => Promise<OpResult<K>> } = {
    "spaces.list": () => invoke("list_spaces"),
    "spaces.create": ({ config, pendingId }) => invoke("create_space", { config, pendingId }),
    "spaces.cancelCreate": ({ pendingId }) => invoke("cancel_create", { pendingId }),
    "spaces.setPower": ({ spaceId, on }) => invoke("set_space_power", { spaceId, on }),
    "spaces.delete": ({ spaceId }) => invoke("delete_space", { spaceId }),
    "spaces.open": async ({ spaceId, name, os }) => {
      await invoke("open_space_window", { space: { id: spaceId, name: name ?? spaceId, os } });
      return null;
    },

    "machines.list": async () => {
      const [status, hosts, env] = await Promise.all([
        invoke<HostStatus>("host_status").catch(() => null),
        invoke<SpaceHost[]>("list_hosts").catch(() => [] as SpaceHost[]),
        invoke<{ platform: string }>("get_environment").catch(() => null),
      ]);
      const selfId = status?.machineId ?? "local";
      const self: MachineRow = {
        id: selfId,
        name: status?.name ?? "This machine",
        via: "local",
        online: true,
        os: env?.platform ?? "macos",
        current: true,
        limits: [],
        host: status,
      };
      return [self, ...hosts.filter((h) => h.id !== selfId)];
    },
    "host.status": () => invoke("host_status"),

    "settings.get": () => readSettings(),
    "settings.set": async ({ key, value }) => {
      switch (key) {
        case "theme":
          await writeUi(UI_STORAGE_KEYS.theme, String(value));
          break;
        case "menuBar":
          await writeUi(UI_STORAGE_KEYS.menuBar, JSON.stringify(Boolean(value)));
          break;
        case "hotkey":
          await writeUi(UI_STORAGE_KEYS.hotkey, String(value));
          break;
        case "telemetry":
          await invoke("telemetry_set_enabled", { enabled: Boolean(value) });
          break;
        case "defaultLocation":
          await invoke("set_default_location", { on: String(value) });
          break;
        case "launchAtLogin":
          await invoke("login_item_set", { on: Boolean(value) });
          break;
        case "updateChannel":
          throw new UnsupportedOperationError("tauri", "settings.set updateChannel");
      }
      return readSettings();
    },

    "keyvault.overview": () => invoke("keyvault_overview"),
    "keyvault.unlock": async ({ passphrase }) => {
      if (passphrase) await invoke("keyvault_unlock_passphrase", { passphrase });
      else await invoke("keyvault_unlock");
      return null;
    },
    "keyvault.setUnattended": ({ itemIds, unattended }) =>
      invoke("keyvault_set_unattended", { args: { itemIds, unattended } }),
    "keyvault.setDisabled": async ({ disabled }) => {
      await invoke("keyvault_set_disabled", { disabled });
      return null;
    },
    "keyvault.approve": ({ requestId, items }) => invoke("keyvault_approve", { requestId, items }),
    "keyvault.deny": async ({ requestId }) => {
      await invoke("keyvault_deny", { requestId });
      return null;
    },
    "keyvault.revokeGrant": ({ id }) => invoke("keyvault_revoke_grant", { id }),

    "session.get": async () => {
      const [fleet, onboarding, daemon] = await Promise.all([
        invoke<FleetStatus>("fleet_status", { probe: false }),
        invoke<OnboardingState>("onboarding_state").catch((): OnboardingState => ({ completed: true })),
        invoke<DaemonStatus>("daemon_status").catch(() => null),
      ]);
      return { fleet, onboarding, daemon };
    },
    "session.signIn": () => invoke("begin_sign_in"),
    "session.signOut": async () => {
      await invoke("sign_out");
      return null;
    },
    "session.completeOnboarding": async ({ mode }) => {
      await invoke("complete_onboarding", { mode });
      return null;
    },
    "session.openExternal": async ({ url }) => {
      await invoke("open_external", { url });
      return null;
    },

    "agents.list": async () => persistentAgentsFromTool(await invoke("agents_tool", { tool: "persistent_agent_list", args: {} })),
    "agents.runs": ({ spaceId }) => invoke<SpaceAgentRun[]>("list_space_agents", { spaceId }),
    "agents.pause": async ({ name }) => {
      await invoke("agents_tool", { tool: "agent_pause", args: { name } });
      return null;
    },
    "agents.resume": async ({ name }) => {
      await invoke("agents_tool", { tool: "agent_resume", args: { name } });
      return null;
    },
    "agents.setup": () => invoke<AgentSetupRow[]>("agent_setup_detect"),
    "agents.configure": ({ agents }) => invoke<AgentSetupRow[]>("agent_setup_configure", { agents }),
    ...newSpaceTauriOps(invoke, readUi),
    ...tauriTeleportOps(invoke, win.__TAURI_INTERNALS__, (e) => events.emit(e)),
    ...tauriShareOps(invoke),
    ...tauriAgentKeysOps(invoke),
    ...tauriKeyvaultSetupOps(invoke),
    ...tauriVolumeOps(invoke, win),
    ...tauriSettingsOps(invoke, { read: readUi, write: writeUi }),
    ...tauriNotificationsOps(invoke),
    ...tauriTelemetryOps(invoke),
    ...tauriSpaceDetailOps(invoke),
    ...tauriHostSetupOps(invoke),
    ...tauriStartupOps(),
  };

  return {
    mode: "tauri",
    pollSpacesMs: listen ? undefined : 10_000,
    call<K extends OpName>(op: K, args: OpArgs<K>) {
      const f = ops[op] as ((a: OpArgs<K>) => Promise<OpResult<K>>) | undefined;
      if (!f) return Promise.reject(new UnsupportedOperationError("tauri", op));
      return f(args);
    },
    subscribe(listener) {
      startListening();
      return events.subscribe(listener);
    },
    dispose() {
      for (const u of unlisteners) void u.then((f) => f()).catch(() => {});
      unlisteners.length = 0;
      listening = false;
      events.clear();
    },
  };
}
