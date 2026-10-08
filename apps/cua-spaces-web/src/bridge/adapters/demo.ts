// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host: realistic data in memory with simulated latency and state
 * changes (creates step through the SDK's phases, power takes a moment,
 * sign-in finishes in the "browser"). Used when no host is detected.
 */

import { Emitter, HostError, UnsupportedOperationError, type DataAdapter } from "../adapter";
import type { AgentSetupRow, PersistentAgent } from "../contracts/agents";
import type { OnboardingState } from "../contracts/host";
import type { KvItem } from "../contracts/keyvault";
import type { SpaceOs, SpaceRow } from "../contracts/spaces";
import {
  DEFAULT_SETTINGS,
  type HostEvent,
  type OpArgs,
  type OpName,
  type OpResult,
  type SessionSnapshot,
  type SettingsSnapshot,
  type SettingsValues,
} from "../protocol";
import {
  DEMO_IDENTITY,
  DEMO_MACHINES,
  demoMachineRows,
  demoKeyvaultItems,
  demoKeyvaultOverview,
  demoSpaceRows,
  demoTelemetry,
} from "./demo-data";
import { demoNewSpaceState, newSpaceDemoHandlers, type DemoNewSpaceState } from "./demo/new-space";
import { demoShareHandlers } from "./demo/share";
import { demoAgentKeysHandlers } from "./demo/agent-keys";
import { demoKeyvaultManageHandlers, demoKeyvaultManageState, withDemoCopies, type DemoKeyvaultManageState } from "./demo/keyvault-manage";
import { demoKeyvaultSetupHandlers, demoKeyvaultSetupState, demoNoVaultOverview, type DemoKeyvaultSetupState } from "./demo/keyvault-setup";
import { unsupportedStreamOps } from "../ops/stream";
import { demoTeleportHandlers } from "./demo/teleport";
import { createDemoSettings } from "./demo/settings";
import { demoAgentSetup, demoPersistentAgents, demoRunRecords, runAt, writtenEvents, type DemoRunRecord } from "./demo-agents";
import { demoVolumeHandlers, demoVolumeState, type DemoVolumeState } from "./demo/volume";
import { demoHostSetupHandlers, demoHostSetupState, type DemoHostSetupState } from "./demo/host-setup";
import { demoSpaceDetailHandlers, demoSpaceDetailState, type DemoSpaceDetailState } from "./demo/space-detail";
import { demoTelemetryHandlers, demoTelemetryState, type DemoTelemetryState } from "./demo/telemetry";
import { demoStartupHandlers, demoStartupState, type DemoStartupState } from "./demo/startup";
import { demoMachine, MAC_DEMO_PLATFORM, type DemoPlatform } from "./demo/platform";

export interface DemoOptions {
  /** Round-trip latency of every call (ms). */
  latencyMs?: number;
  /** Time per create phase and per power change (ms). */
  stepMs?: number;
  /** Clock (tests). */
  now?: () => number;
  signedIn?: boolean;
  onboarded?: boolean;
  keyvaultLocked?: boolean;
  /** Start with no Keyvault (`?demo=novault`). */
  noVault?: boolean;
  /** Start with no Spaces (the first-Space empty state). */
  noSpaces?: boolean;
  /** Start at the macOS Keychain prompt (the startup screen). */
  keychain?: boolean;
  /** Answer New Space's options as the SwiftUI host does (`demo/new-space.ts`). */
  macHost?: boolean;
  /** The machine the demo runs on: a Mac on Apple silicon unless set (the
   * Electron shell passes its own system and architecture). */
  platform?: DemoPlatform;
}

/** `?demo=fresh` (signed out, first run), `?demo=locked` (Keyvault locked), `?demo=novault` (no Keyvault yet),
 * `?demo=empty` (no Spaces yet), `?demo=keychain` (the startup screen) and
 * `?demo=mac-host` (New Space options in the SwiftUI host's shape). */
export function demoOptionsFromSearch(search: string): DemoOptions {
  const flags = new Set((new URLSearchParams(search).get("demo") ?? "").split(",").filter(Boolean));
  return {
    signedIn: !flags.has("fresh"),
    onboarded: !flags.has("fresh"),
    keyvaultLocked: flags.has("locked"),
    noVault: flags.has("novault"),
    noSpaces: flags.has("empty"),
    keychain: flags.has("keychain"),
    macHost: flags.has("mac-host"),
  };
}

const CREATE_PHASES = ["preparing", "pulling", "creating", "booting", "waiting_for_services", "connecting"];

type Handler<K extends OpName> = (args: OpArgs<K>) => Promise<OpResult<K>> | OpResult<K>;

export function createDemoAdapter(options: DemoOptions = {}): DataAdapter & { readonly state: DemoState } {
  const latency = options.latencyMs ?? 120;
  const step = options.stepMs ?? 700;
  const now = options.now ?? Date.now;
  const events = new Emitter();
  const timers = new Set<ReturnType<typeof setTimeout>>();

  const state: DemoState = {
    rows: options.noSpaces ? [] : demoSpaceRows(now()),
    items: demoKeyvaultItems(now()),
    keyvaultLocked: Boolean(options.keyvaultLocked),
    keyvaultDisabled: false,
    pendingRequests: true,
    revokedGrants: new Set(),
    settings: { ...DEFAULT_SETTINGS, launchAtLogin: true, updateChannel: "stable" },
    identity: options.signedIn === false ? undefined : DEMO_IDENTITY,
    onboarding: { completed: options.onboarded !== false, mode: options.onboarded === false ? null : "client" },
    creates: new Map(),
    agents: demoPersistentAgents(now()),
    runs: demoRunRecords(now()),
    agentSetup: demoAgentSetup(),
    platform: options.platform ?? MAC_DEMO_PLATFORM,
    volume: demoVolumeState(now(), options.onboarded !== false, options.platform),
    ...demoTelemetryState(),
    ...demoSpaceDetailState(),
    ...demoHostSetupState(now(), options.onboarded === false),
    ...demoStartupState(Boolean(options.keychain)),
    ...demoNewSpaceState(Boolean(options.macHost)),
    ...demoKeyvaultSetupState(!options.noVault),
    ...demoKeyvaultManageState(),
  };

  if (state.platform.os !== "macos") state.host = { ...state.host, name: demoMachine(state.platform).name };

  const later = (ms: number, f: () => void) => {
    const t = setTimeout(() => {
      timers.delete(t);
      f();
    }, ms);
    timers.add(t);
    return t;
  };
  const wait = (ms: number) => new Promise<void>((r) => later(ms, r));
  const emit = (e: HostEvent) => events.emit(e);
  const findRow = (id: string) => {
    const row = state.rows.find((r) => r.id === id);
    if (!row) throw new HostError(`no Space ${id}`, "not_found");
    return row;
  };
  const replaceRow = (id: string, patch: Partial<SpaceRow>) => {
    state.rows = state.rows.map((r) => (r.id === id ? { ...r, ...patch } : r));
  };
  const settingsSnapshot = (): SettingsSnapshot => ({
    values: { ...state.settings },
    defaultLocation: { value: state.settings.defaultLocation, source: "config", path: "~/.cua/config.toml" },
    telemetry: demoTelemetry(state.settings.telemetry),
  });
  const overview = () => {
    if (!state.keyvaultSetUp) return demoNoVaultOverview(now());
    const o = demoKeyvaultOverview(now(), state.items, state.keyvaultLocked);
    if (o.status) o.status.disabled = state.keyvaultDisabled;
    if (!state.pendingRequests) {
      o.pending = [];
      if (o.status) o.status.pending = 0;
    }
    o.grants = o.grants.filter((g) => !state.revokedGrants.has(g.id));
    return withDemoCopies(o, state, now());
  };
  const session = (): SessionSnapshot => ({
    fleet: {
      configured: Boolean(state.identity),
      authMode: state.identity ? "user" : "none",
      baseUrl: "https://api.cua.ai",
      tokenUrl: "https://auth.cua.ai/oauth/token",
      identity: state.identity,
      namespaces: state.identity ? ["ada"] : [],
    },
    onboarding: { ...state.onboarding },
    daemon: { connected: true, version: "0.6.0", socketPath: "~/.cua/daemon.sock" },
  });

  const findAgent = (name: string) => {
    const agent = state.agents.find((a) => a.name === name);
    if (!agent) throw new HostError(`no persistent agent named ${name}`, "not_found");
    return agent;
  };
  const replaceAgent = (name: string, patch: Partial<PersistentAgent>) => {
    state.agents = state.agents.map((a) => (a.name === name ? { ...a, ...patch } : a));
  };

  const ctx = { state, now, wait, emit, findRow, stepMs: step };
  const handlers: { [K in OpName]: Handler<K> } = {
    "spaces.list": () => state.rows.map((r) => ({ ...r })),

    "spaces.create": async ({ config, pendingId }) => {
      const os: SpaceOs = /mac/i.test(config.image ?? "") ? "macos" : /win/i.test(config.image ?? "") ? "windows" : "linux";
      const on = config.on ?? state.settings.defaultLocation;
      const host = on.startsWith("host:") ? DEMO_MACHINES.find((m) => m.id === on.slice(5)) : undefined;
      const base = (config.name?.trim() || `${os === "macos" ? "mac" : os}-${Math.random().toString(36).slice(2, 6)}`)
        .toLowerCase()
        .replace(/[^a-z0-9-]+/g, "-");
      const id = host ? `relay:${host.id}/${base}` : `local:${base}`;
      const job = { cancelled: false };
      state.creates.set(pendingId, job);
      try {
        for (const phase of CREATE_PHASES) {
          for (const fraction of [0, 0.5]) {
            if (job.cancelled) throw new HostError("cancelled: the create was stopped and what it made was removed", "cancelled");
            emit({
              type: "spaces.createProgress",
              progress: {
                pendingId,
                phase,
                fraction,
                detail: phase,
                space: id,
                ...(phase === "pulling" ? { bytesDone: fraction * 4.2e9, bytesTotal: 4.2e9, bytesPerSecond: 85e6 } : {}),
              },
            });
            await wait(step / 2);
          }
        }
        if (job.cancelled) throw new HostError("cancelled: the create was stopped and what it made was removed", "cancelled");
        const row: SpaceRow = {
          id,
          name: base,
          provider: host ? "relay" : "local",
          spacesdVersion: "0.6.0",
          features: ["desktop_stream", "window_stream", "audio.desktop"],
          addedAt: new Date(now()).toISOString(),
          os,
          image: config.image,
          kind: os === "linux" ? "container" : "vm",
          arch: "arm64",
          reachable: true,
          host: host?.id,
          hostName: host?.name,
          power: os === "linux" ? "stop" : "suspend",
          powerState: "running",
        };
        state.rows = [...state.rows, row];
        emit({ type: "spaces.changed" });
        return row;
      } finally {
        state.creates.delete(pendingId);
      }
    },

    "spaces.cancelCreate": async ({ pendingId }) => {
      const job = state.creates.get(pendingId);
      if (!job) return { id: pendingId, state: "not_creating", message: "Nothing was being created." };
      job.cancelled = true;
      return { id: pendingId, state: "cancelled", message: "Stopped. Nothing was left behind." };
    },

    "spaces.setPower": async ({ spaceId, on }) => {
      const row = findRow(spaceId);
      if (!row.power) throw new HostError("This Space can't be turned off.", "unsupported");
      await wait(step * 2);
      const next = on ? "running" : row.power === "stop" ? "stopped" : "suspended";
      replaceRow(spaceId, { powerState: next, reachable: on });
      emit({ type: "spaces.changed" });
      return { space: spaceId, state: next, power: row.power, message: on ? "Started" : row.power === "stop" ? "Stopped" : "Suspended" };
    },

    "spaces.delete": async ({ spaceId }) => {
      findRow(spaceId);
      await wait(step * 2);
      state.rows = state.rows.filter((r) => r.id !== spaceId);
      emit({ type: "spaces.changed" });
      return spaceId;
    },

    // There is no window to open in the browser; the Space only has to exist.
    "spaces.open": ({ spaceId }) => {
      findRow(spaceId);
      return null;
    },

    "machines.list": () => demoMachineRows(now(), state.host, state.platform),
    "host.status": () => ({ ...state.host }),

    "settings.get": () => settingsSnapshot(),
    "settings.set": ({ key, value }) => {
      state.settings = { ...state.settings, [key]: value } as SettingsValues;
      emit({ type: "settings.changed" });
      return settingsSnapshot();
    },
    // The demo lays out none of the host's own rows: nothing to change.
    "settings.choose": () => settingsSnapshot(),

    "keyvault.overview": () => overview(),
    "keyvault.unlock": async ({ passphrase }) => {
      if (passphrase !== undefined && passphrase !== null && passphrase.length < 4) {
        throw new HostError("That passphrase doesn't unlock this Keyvault.", "bad_passphrase");
      }
      await wait(step);
      state.keyvaultLocked = false;
      emit({ type: "keyvault.changed" });
      return null;
    },
    "keyvault.setUnattended": ({ itemIds, unattended }) => {
      const idp = state.items.find((i) => itemIds.includes(i.id) && i.identity_provider);
      if (unattended && idp) throw new HostError(`${idp.domain ?? idp.app_display} signs you in elsewhere, so it never runs unattended.`, "identity_provider");
      state.items = state.items.map((i): KvItem =>
        itemIds.includes(i.id) ? { ...i, policy: { ...i.policy, unattended }, rev: i.rev + 1, updated_ms: now() } : i,
      );
      emit({ type: "keyvault.changed" });
      return state.items.filter((i) => itemIds.includes(i.id));
    },
    "keyvault.setDisabled": ({ disabled }) => {
      state.keyvaultDisabled = disabled;
      emit({ type: "keyvault.changed" });
      return null;
    },
    "keyvault.approve": ({ requestId, items }) => {
      state.pendingRequests = false;
      emit({ type: "keyvault.changed" });
      return {
        id: `grant-${requestId}`,
        request_id: requestId,
        caller_fp: "fp-claude",
        caller_display: "Claude Code",
        items: items ?? ["kv-gh-session", "kv-gh-sess", "kv-gh-theme"],
        targets: ["local:design-review"],
        actions: ["deliver"],
        created_ms: now(),
        not_after_ms: now() + 3_600_000,
        revoked: false,
      };
    },
    "keyvault.deny": () => {
      state.pendingRequests = false;
      emit({ type: "keyvault.changed" });
      return null;
    },
    "keyvault.revokeGrant": ({ id }) => {
      state.revokedGrants.add(id);
      emit({ type: "keyvault.changed" });
      return 1;
    },

    "session.get": () => session(),
    "session.signIn": () => {
      later(step * 3, () => {
        state.identity = DEMO_IDENTITY;
        emit({ type: "session.signedIn", identity: DEMO_IDENTITY });
      });
      return { method: "browser", verificationUri: "https://cua.ai/signin" };
    },
    "session.signOut": () => {
      state.identity = undefined;
      emit({ type: "session.signedOut" });
      return null;
    },
    "session.completeOnboarding": ({ mode }) => {
      state.onboarding = { ...state.onboarding, completed: true, mode };
      return null;
    },
    "session.openExternal": ({ url }) => {
      globalThis.window?.open?.(url, "_blank", "noopener");
      return null;
    },

    "agents.list": () => state.agents.map((a) => ({ ...a })),
    "agents.runs": ({ spaceId }) => {
      const row = findRow(spaceId);
      if (row.powerState !== "running" || !row.reachable) throw new HostError(`${row.name} is not running`, "space_off");
      return state.runs
        .filter((r) => r.space === spaceId)
        .map((r) => runAt(r, now()))
        .sort((a, b) => (b.createdAt ?? 0) - (a.createdAt ?? 0));
    },
    "agents.events": ({ spaceId, runId, cursor, max }) => {
      const rec = state.runs.find((r) => r.space === spaceId && r.run.runId === runId);
      if (!rec) throw new HostError(`no run ${runId} in ${spaceId}`, "not_found");
      const written = writtenEvents(rec, now());
      const after = written.filter((e) => e.seq > cursor);
      const page = after.slice(0, max ?? 100);
      const run = runAt(rec, now());
      return {
        run_id: runId,
        status: run.status,
        phase: run.phase,
        events: page,
        cursor: page.length ? page[page.length - 1]!.seq : cursor,
        caught_up: page.length === after.length,
      };
    },
    "agents.pause": async ({ name }) => {
      const agent = findAgent(name);
      await wait(step);
      const rec = state.runs.find((r) => r.run.runId === agent.runId);
      if (rec && rec.live && rec.stoppedAt === undefined && writtenEvents(rec, now()).length < rec.events.length) {
        // Pausing stops the run: what was not written yet never will be.
        const t = now();
        const written = writtenEvents(rec, t);
        const last = written[written.length - 1];
        rec.events = [
          ...written,
          { seq: (last?.seq ?? 0) + 1, ts_ms: t, turn: last?.turn ?? 0, kind: "turn_ended", stop_reason: "cancelled", category: "activity", summary: `Turn ${last?.turn ?? 0} ended (cancelled)` },
        ];
        rec.stoppedAt = t;
      }
      const local = agent.space.startsWith("local:");
      replaceAgent(name, { paused: true, runId: null, spaceState: local ? "suspended" : "released", savedMs: now() });
      emit({ type: "agents.changed" });
      return null;
    },
    "agents.resume": async ({ name }) => {
      findAgent(name);
      await wait(step);
      replaceAgent(name, { paused: false, spaceState: "running" });
      emit({ type: "agents.changed" });
      return null;
    },
    "agents.setup": () => state.agentSetup.map((r) => ({ ...r })),
    "agents.configure": async ({ agents }) => {
      await wait(step);
      const wanted = (r: AgentSetupRow) => r.installed && (agents === null || agents.includes(r.agent));
      state.agentSetup = state.agentSetup.map((r) =>
        wanted(r) ? { ...r, configured: true, skillsInstalled: r.skillsTotal, detail: "Skills and MCP server set up" } : r,
      );
      return state.agentSetup.map((r) => ({ ...r }));
    },
    ...newSpaceDemoHandlers({ state, wait, emit, step, now }),
    ...demoTeleportHandlers({ wait, emit, now, stepMs: step }),
    ...demoShareHandlers({ wait, stepMs: step }),
    ...demoAgentKeysHandlers({ wait, stepMs: step, now }),
    ...demoVolumeHandlers({ state: state.volume, now, wait, step, emit, platform: state.platform }),
    ...createDemoSettings({ now, wait, emit, stepMs: step, platform: state.platform, experiments: state.volume.experiments, signedIn: () => Boolean(state.identity), settings: () => state.settings, setSettings: (p) => ((state.settings = { ...state.settings, ...p }), emit({ type: "settings.changed" })) }),
    ...demoTelemetryHandlers(ctx),
    ...demoSpaceDetailHandlers(ctx),
    ...demoHostSetupHandlers(ctx),
    ...demoKeyvaultSetupHandlers(ctx),
    ...demoKeyvaultManageHandlers(ctx, overview),
    ...demoStartupHandlers(ctx),
    ...unsupportedStreamOps("demo"),
  };

  return {
    mode: "demo",
    state,
    async call<K extends OpName>(op: K, args: OpArgs<K>): Promise<OpResult<K>> {
      const handler = handlers[op] as Handler<K> | undefined;
      if (!handler) throw new UnsupportedOperationError("demo", op);
      await wait(latency);
      return handler(args);
    },
    subscribe: (l) => events.subscribe(l),
    dispose() {
      for (const t of timers) clearTimeout(t);
      timers.clear();
      events.clear();
    },
  };
}

export interface DemoState extends DemoTelemetryState, DemoSpaceDetailState, DemoHostSetupState, DemoStartupState, DemoNewSpaceState, DemoKeyvaultSetupState, DemoKeyvaultManageState {
  rows: SpaceRow[];
  items: KvItem[];
  keyvaultLocked: boolean;
  keyvaultDisabled: boolean;
  pendingRequests: boolean;
  revokedGrants: Set<string>;
  settings: SettingsValues;
  identity: string | undefined;
  onboarding: OnboardingState;
  creates: Map<string, { cancelled: boolean }>;
  agents: PersistentAgent[];
  runs: DemoRunRecord[];
  agentSetup: AgentSetupRow[];
  volume: DemoVolumeState;
  platform: DemoPlatform;
}
