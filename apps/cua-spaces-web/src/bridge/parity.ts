// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The app core's parity flows (cua-spaces-app-core/parity), replayed
 * through this bridge. Each core call a flow makes that the bridge also
 * makes for a screen goes through the bridge's own function (`derive.ts`,
 * or the store's call); the rest go to the core. The transcript must equal
 * the golden the Rust core, the Tauri webview and the SwiftUI app match.
 *
 * Along the way the replay records the states the screens draw
 * (`checkpoints`), so the Playwright harness (`e2e/parity.spec.ts`) can put
 * each one on the real screen and check what it shows. The harness reaches
 * this through `window.__cuaParity`, installed only in demo mode with
 * `?parity` in the URL.
 */

import type { CoreClient } from "./core";
import type { PersistentAgent } from "./contracts/agents";
import type { HostPanelView, MachineRow } from "./contracts/host";
import type { KeyvaultOverview, KvPage, VaultState, VaultView } from "./contracts/keyvault";
import type { CreatesState, Space, SpaceRow } from "./contracts/spaces";
import {
  NO_CREATES,
  agentsPageInitial,
  agentsPageView,
  composeCreates,
  hostPanelFromState,
  keyvaultPane,
  keyvaultViews,
  mergeMachines,
  reduceCreates,
  rowsToSpaces,
  validateImageRef,
  type AgentRowView,
  type MachinesInput,
  type MergedMachine,
} from "./derive";
import type { HostFormState } from "./ops/host-setup";
import type { SpaceDetailView, SpaceUsage, StreamSection, StreamSectionInput } from "./ops/space-detail";
import type { TelemetrySignal } from "./ops/telemetry";
import { detailCopy, pipClick, pipReduce, spaceDetail, streamSection } from "./space-detail";
import { newSpaceParityHandle, newSpaceParityMethods, type NewSpaceCheckpoint, type NewSpaceParityHandle } from "./new-space";
import { teleportParityHandle, teleportParityMethods, type TeleportCheckpoint, type TeleportParityHandle } from "./parity-teleport";
import { settingsParityHandle, settingsParityMethods, type SettingsCheckpoint, type SettingsParityHandle } from "./parity-settings";
import { agentKeysParityHandle, agentKeysParityMethods, type AgentKeysCheckpoint, type AgentKeysParityHandle } from "./parity-agent-keys";
import type { BridgeStore } from "./store";
import { volumeBridgeMethods, volumeParityHandle, type VolumeCheckpoint, type VolumeParityHandle } from "./parity-volume";
import {
  telemetryCreates,
  telemetryEnroll,
  telemetryLaunched,
  telemetryOnboarding,
  telemetryOnboardingFinished,
  telemetryShare,
  telemetryStorage,
} from "./telemetry";
import { hostFormInitial, hostFormReduce, hostFormView, type HostFormView } from "./this-machine";

type Args = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

/** A state a screen draws, as the replay reached it. */
export type ParityCheckpoint =
  | {
      kind: "spaces";
      /** The registry and create state the Spaces screen composes. */
      registry: Space[];
      creates: CreatesState;
      /** What `creates.compose` returned: the Spaces the screen lists. */
      spaces: Space[];
    }
  | { kind: "keyvault-page"; overview: KeyvaultOverview; page: KvPage }
  | { kind: "keyvault-vault"; overview: KeyvaultOverview; state: VaultState; vault: VaultView }
  | NewSpaceCheckpoint
  | TeleportCheckpoint
  | VolumeCheckpoint
  | SettingsCheckpoint
  | AgentKeysCheckpoint
  /** A Space's detail (`sidebar.detail`) for this Space, usage and host architecture. */
  | { kind: "space-detail"; space: Space; usage: SpaceUsage | null; hostArch: string | null; detail: SpaceDetailView }
  /** A Stream section for this input. */
  | { kind: "stream"; input: StreamSectionInput; section: StreamSection }
  /** This machine for the core's host state (null: not reported yet). */
  | { kind: "host-panel"; state: object | null; panel: HostPanelView }
  /** The host setup form for this state and signed-in identity. */
  | { kind: "host-form"; state: HostFormState; identity: string | null; view: HostFormView }
  /** The Agents page's rows for these agents. */
  | { kind: "agents"; agents: PersistentAgent[]; nowMs: number; rows: AgentRowView[] }
  /** The Machines page for these host rows at `now` (Unix seconds). */
  | { kind: "machines"; rows: MachineRow[]; now: number; merged: MergedMachine[] }
  /** Usage events a step means (empty: none). */
  | { kind: "telemetry"; method: string; signals: TelemetrySignal[] };

export interface ParityReplay {
  name: string;
  transcript: unknown;
  golden: unknown;
  /** Core methods the bridge answered with its own functions. */
  bridged: string[];
  /** Core methods that went straight to the core. */
  direct: string[];
  checkpoints: ParityCheckpoint[];
}

/**
 * The bridge's answer to each core method it calls for a screen. `record`
 * gets the states the screens draw.
 */
function bridgeMethods(core: CoreClient, record: (c: ParityCheckpoint) => void): Record<string, (a: Args) => unknown> {
  const views = (a: Args) => keyvaultViews(core, a.overview, a.now, a.state)!;
  return {
    "spaces.rowsToSpaces": (a) => rowsToSpaces(core, a.rows as SpaceRow[], a.now),
    "creates.reduce": (a) => reduceCreates(core, a.state, a.action),
    "creates.compose": (a) => {
      const spaces = composeCreates(core, a.spaces, a.state);
      record({ kind: "spaces", registry: a.spaces, creates: a.state, spaces });
      return spaces;
    },
    // The store's own calls (store.ts: applyRows, vaultAction).
    "creates.settle": (a) => core.tryCall("creates.settle", a),
    "keyvault.vaultReduce": (a) => core.tryCall("keyvault.vaultReduce", a),
    "wizard.validateImageRef": (a) => validateImageRef(core, a.ref),
    "keyvault.page": (a) => {
      const page = views(a).page;
      record({ kind: "keyvault-page", overview: a.overview, page });
      return page;
    },
    "keyvault.sidebar": (a) => views(a).sidebar,
    "keyvault.vaultView": (a) => {
      const vault = views(a).vault;
      record({ kind: "keyvault-vault", overview: a.overview, state: a.state, vault });
      return vault;
    },
    "keyvault.list": (a) => keyvaultPane(core, a.overview, a.selection, a.now),
    ...newSpaceParityMethods(core, record),
    ...teleportParityMethods(core, record),
    ...volumeBridgeMethods(core, record),
    ...settingsParityMethods(core, record),
    ...agentKeysParityMethods(core, record),

    // A Space's detail (space-detail.ts).
    "sidebar.detail": (a) => {
      const detail = spaceDetail(core, a.space, a.usage, a.hostArch, a.experiments);
      record({ kind: "space-detail", space: a.space, usage: a.usage ?? null, hostArch: a.hostArch ?? null, detail });
      return detail;
    },
    "sidebar.detailCopy": () => detailCopy(core),
    "sidebar.streamSection": (a) => {
      const section = streamSection(core, a.input);
      record({ kind: "stream", input: a.input, section });
      return section;
    },
    "stream.pipReduce": (a) => pipReduce(core, a.open, a.event),
    "stream.pipClick": (a) => pipClick(core, a.open, a.row),

    // This machine and its setup form (derive.ts, this-machine.ts).
    "host.panel": (a) => {
      const panel = hostPanelFromState(core, a.state);
      record({ kind: "host-panel", state: a.state, panel });
      return panel;
    },
    "host.formInitial": () => hostFormInitial(core),
    "host.formReduce": (a) => hostFormReduce(core, a.state, a.action),
    "host.formView": (a) => {
      const view = hostFormView(core, a.state, a.identity)!;
      record({ kind: "host-form", state: a.state, identity: a.identity ?? null, view });
      return view;
    },

    // The Machines page's list (derive.ts: mergeMachineRows).
    "machines.merge": (a) => {
      const input = a.input as MachinesInput;
      const merged = mergeMachines(core, input);
      record({ kind: "machines", rows: machineRowsOf(input), now: input.now, merged });
      return merged;
    },

    // The Agents page's rows (derive.ts: agentRows).
    "agents.pageInitial": () => agentsPageInitial(core),
    "agents.pageView": (a) => {
      const view = agentsPageView(core, a.input, a.state, a.nowMs)!;
      record({ kind: "agents", agents: a.input.agents ?? [], nowMs: a.nowMs, rows: view.rows });
      return view;
    },

    // Usage events (telemetry.ts).
    ...telemetryMethods(core, (method, signals) => record({ kind: "telemetry", method, signals })),
  };
}

/** A flow's machines and devices as a host reports them (`machines.list`). */
function machineRowsOf(input: MachinesInput): MachineRow[] {
  return [
    ...input.machines.map(
      (m): MachineRow => ({
        id: m.id,
        name: m.name,
        via: m.current ? "local" : "relay",
        online: m.online,
        os: m.os,
        limits: [],
        ...(m.current ? { current: true } : {}),
        ...(m.presence != null ? { presence: m.presence } : {}),
        ...(m.hostname ? { hostname: m.hostname } : {}),
      }),
    ),
    ...input.devices.map(
      (d): MachineRow => ({
        id: d.id,
        name: d.name,
        via: "relay",
        online: false,
        os: d.platform,
        limits: [],
        device: true,
        deviceState: d.state,
        lastSeen: d.lastSeen,
        ...(d.current ? { current: true } : {}),
      }),
    ),
  ];
}

function telemetryMethods(core: CoreClient, rec: (method: string, s: TelemetrySignal[]) => void): Record<string, (a: Args) => unknown> {
  const out = (method: string, signals: TelemetrySignal[]) => {
    rec(method, signals);
    return signals;
  };
  return {
    "telemetry.launched": (a) => out("telemetry.launched", telemetryLaunched(core, (a.onboardingEligible as boolean | null | undefined) ?? null)),
    "telemetry.onboarding": (a) => out("telemetry.onboarding", telemetryOnboarding(core, a.state, a.action)),
    "telemetry.onboardingFinished": (a) => out("telemetry.onboardingFinished", telemetryOnboardingFinished(core, a.state)),
    "telemetry.creates": (a) => out("telemetry.creates", telemetryCreates(core, a.state, a.action, a.now)),
    "telemetry.storage": (a) => out("telemetry.storage", telemetryStorage(core, a.input, a.state, a.action)),
    "telemetry.share": (a) => out("telemetry.share", telemetryShare(core, a.input, a.state, a.action)),
    "telemetry.enroll": (a) => out("telemetry.enroll", telemetryEnroll(core, a.state, a.action)),
  };
}

/** Replays one flow through the bridge. Throws when the core lacks the harness. */
export function replayFlow(core: CoreClient, name: string): ParityReplay {
  const parity = core.parity;
  if (!parity) throw new Error(`the app core does not export the parity flows (status: ${core.status})`);
  const flow = parity.flows().find((f) => f.name === name);
  if (!flow) throw new Error(`no parity flow named ${name}`);
  const checkpoints: ParityCheckpoint[] = [];
  const methods = bridgeMethods(core, (c) => checkpoints.push(c));
  const bridged = new Set<string>();
  const direct = new Set<string>();
  const transcript = parity.run(flow.name, flow.flow, (method, args) => {
    const viaBridge = methods[method];
    if (viaBridge) {
      bridged.add(method);
      return viaBridge(args);
    }
    direct.add(method);
    return core.call(method, args);
  });
  return {
    name,
    transcript,
    golden: JSON.parse(flow.golden),
    bridged: [...bridged],
    direct: [...direct],
    checkpoints,
  };
}

/** What the Playwright harness calls (`window.__cuaParity`). */
export interface ParityHandle extends NewSpaceParityHandle, TeleportParityHandle, VolumeParityHandle, AgentKeysParityHandle {
  coreStatus: CoreClient["status"];
  /** Every flow's name, in the core's order. */
  flows(): string[];
  replay(name: string): ParityReplay;
  /** Puts a Spaces checkpoint on the screen through the store. */
  showSpaces(registry: Space[], creates: CreatesState): void;
  /** Puts a Keyvault overview on the screen through the store. */
  showKeyvault(overview: KeyvaultOverview): void;
  /** Puts Settings and Notifications states on the screen (`parity-settings.ts`). */
  settings: SettingsParityHandle;
  /** Lists only this Space, with these readings for its detail. */
  showSpaceDetail(space: Space, usage: SpaceUsage | null, hostArch: string | null): void;
  /** Lists one running Space with this Stream section input; returns its id. */
  showStream(input: StreamSectionInput): string;
  /** This machine for the core's host state. */
  showHost(state: object | null): void;
  /** The host setup form in this state, for `identity`. */
  showHostForm(state: HostFormState, identity: string | null): void;
  /** These machine and device rows on the Machines page, at `now` (Unix seconds). */
  showMachines(rows: MachineRow[], now: number): void;
  /** These persistent agents on the Agents page. */
  showAgents(agents: PersistentAgent[], nowMs: number): void;
  /** Sends usage events through the bridge; resolves once sent or dropped. */
  track(signals: TelemetrySignal[]): Promise<void>;
  /** What the demo host's telemetry recorded, in order. */
  tracked(): TelemetrySignal[];
  /** The machine's usage-data setting, through the bridge. */
  setTelemetry(on: boolean): Promise<void>;
  /** Client-side navigation (the store and what it shows stay). */
  navigate(path: string): void;
}

declare global {
  interface Window {
    __cuaParity?: ParityHandle;
  }
}

/** Installs `window.__cuaParity` when the page asks for it (`?parity`). */
export function installParityHandle(store: BridgeStore): () => void {
  if (typeof window === "undefined" || store.adapter.mode !== "demo") return () => {};
  if (!new URLSearchParams(window.location.search).has("parity")) return () => {};
  const core = store.core;
  window.__cuaParity = {
    coreStatus: core.status,
    flows: () => core.parity?.flows().map((f) => f.name) ?? [],
    replay: (name) => replayFlow(core, name),
    showSpaces: (registry, creates) => store.showSpaces(registry, creates),
    showKeyvault: (overview) => store.showKeyvault(overview),
    ...newSpaceParityHandle,
    ...teleportParityHandle(store),
    ...volumeParityHandle(),
    ...agentKeysParityHandle(store),
    settings: settingsParityHandle(store),
    showSpaceDetail: (space, usage, hostArch) => {
      store.showSpaces([space], NO_CREATES);
      store.details.show(space.id, { usage, hostArch, windows: [] });
    },
    showStream: (input) => {
      const row: SpaceRow = {
        id: "local:stream-parity",
        name: "stream-parity",
        provider: "local",
        spacesdVersion: "0.4.0",
        features: ["desktop_stream", "window_stream"],
        os: input.os,
        osName: input.osName ?? undefined,
        reachable: true,
      };
      const space = rowsToSpaces(core, [row], Date.now())[0]!;
      store.showSpaces([space], NO_CREATES);
      store.details.show(space.id, {
        windows: input.windows,
        failed: Boolean(input.failed),
        display: input.display ?? null,
        open: input.open ?? [],
        query: input.query ?? "",
      });
      return space.id;
    },
    showHost: (state) => store.showHost(state),
    showHostForm: (state, identity) => store.thisMachine.show(state, identity),
    showMachines: (rows, now) => store.showMachines(rows, now),
    showAgents: (agents, nowMs) => store.showAgents(agents, nowMs),
    track: async (signals) => {
      await store.telemetry.track(signals);
    },
    tracked: () => [...((store.adapter as { state?: { telemetry?: TelemetrySignal[] } }).state?.telemetry ?? [])],
    setTelemetry: (on) => store.updateSetting("telemetry", on),
    navigate: (path) => {
      window.history.pushState(window.history.state, "", path);
      window.dispatchEvent(new PopStateEvent("popstate", { state: window.history.state }));
    },
  };
  return () => {
    delete window.__cuaParity;
  };
}
