// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Electron host of the web UI's native bridge: the SwiftUI host's
// `WebUIBridge`, ported. One file per area registers its methods under the
// SwiftUI host's names (`methods.ts`); this puts them in one table and tells
// the page when what it shows changed (`cua:event`: `spaces.changed`,
// `machines.changed`, `keyvault.changed`, `session.changed`, `settings.changed`,
// `startup.changed`, `spaces.createProgress`). `ipc.ts` carries it over
// Electron's IPC.
import { aboutMethods } from "./about";
import { agentKeysMethods } from "./agent-keys";
import { agentsMethods, agentsSignature } from "./agents";
import { appMethods } from "./app";
import { cloudsMethods } from "./clouds";
import type { BridgeContext } from "./context";
import { devicesMethods } from "./devices";
import { filesMethods } from "./files";
import { BridgeEvents, BridgeRegistry } from "./host";
import { hostSetupMethods } from "./host-setup";
import { keyvaultMethods } from "./keyvault";
import { hostStatus, machinesMethods } from "./machines";
import { ELECTRON_METHODS, METHODS } from "./methods";
import { notificationsMethods } from "./notifications";
import { onboardingMethods } from "./onboarding";
import { sessionMethods, sessionState } from "./session";
import { settingsMethods } from "./settings";
import { sharingMethods } from "./sharing";
import { spaceDetailMethods } from "./space-detail";
import { spacesMethods } from "./spaces";
import { followStartup, startupMethods } from "./startup";
import { storageMethods } from "./storage";
import { streamMethods } from "./stream";
import { telemetryMethods } from "./telemetry";
import { teleportMethods } from "./teleport";
import { encode } from "./value";
import { volumeMethods } from "./volume";
import { windowMethods } from "./window";

export const AREAS = [
  appMethods,
  sessionMethods,
  spacesMethods,
  machinesMethods,
  agentsMethods,
  agentKeysMethods,
  cloudsMethods,
  teleportMethods,
  sharingMethods,
  volumeMethods,
  aboutMethods,
  devicesMethods,
  storageMethods,
  notificationsMethods,
  telemetryMethods,
  spaceDetailMethods,
  filesMethods,
  hostSetupMethods,
  settingsMethods,
  keyvaultMethods,
  windowMethods,
  startupMethods,
  streamMethods,
  onboardingMethods,
];

export interface Bridge {
  registry: BridgeRegistry;
  events: BridgeEvents;
  /** What the areas work with (the app's background work starts from it, `host-start.ts`). */
  context: BridgeContext;
  /** Stops following the model. */
  stop(): void;
}

/** The method table in the SwiftUI host's order (`app.info` lists it), then this host's own. */
export function orderedMethods(registry: BridgeRegistry): string[] {
  const order = [...METHODS, ...ELECTRON_METHODS] as readonly string[];
  return [...registry.methods].sort((a, b) => order.indexOf(a) - order.indexOf(b));
}

export function createBridge(base: Omit<BridgeContext, "events" | "methods">): Bridge {
  const registry = new BridgeRegistry();
  const events = new BridgeEvents();
  const ctx: BridgeContext = { ...base, events, methods: () => orderedMethods(registry) };
  for (const area of AREAS) registry.register(area(ctx));
  const stops = [followStartup(ctx), followModel(ctx)];
  return { registry, events, context: ctx, stop: () => stops.forEach((s) => s()) };
}

/**
 * Tells the page when what it shows changed (it asks again). Each event goes
 * out only when its answer differs, once per turn of the event loop (a timer: what changes together goes out once).
 */
function followModel(ctx: BridgeContext): () => void {
  const { model, events } = ctx;
  const read: Record<string, () => unknown> = {
    // The Spaces themselves (their details follow from them; "last used"
    // moves with the clock on every read, and the page reads it again with
    // the next change).
    "spaces.changed": () => [encode(model.spaces.map(({ lastUsedAt: _, ...s }) => s)), model.selectedSpaceId, model.loaded, model.creates.deleting.map((d) => d.id), model.rosterError, model.daemonNotice],
    "session.changed": () => sessionState(model),
    "settings.changed": () => [encode(model.settings), model.lumeSource, model.linuxSource],
    "machines.changed": () => [hostStatus(model), model.devices.snapshot === null ? null : encode(model.devices.view)],
    "agents.changed": () => agentsSignature(model),
    // The broker's overview, whether an action is running, and the copies dismissed from the notch.
    "keyvault.changed": () => [encode(model.keyvault.overview), model.keyvault.busy, model.keyvault.dismissed],
  };
  const byChange: Record<string, string> = {
    spaces: "spaces.changed",
    session: "session.changed",
    settings: "settings.changed",
    machines: "machines.changed",
    agents: "agents.changed",
    keyvault: "keyvault.changed",
  };
  const last = new Map<string, string>(Object.entries(read).map(([e, f]) => [e, signature(f())]));
  const due = new Set<string>();
  let scheduled = false;
  const flush = () => {
    scheduled = false;
    for (const event of [...due]) {
      due.delete(event);
      const now = signature(read[event]!());
      if (last.get(event) === now) continue;
      last.set(event, now);
      events.emit(event);
    }
  };
  return model.subscribe((change) => {
    const event = byChange[change];
    if (!event) return;
    due.add(event);
    if (!scheduled) {
      scheduled = true;
      setTimeout(flush, 0);
    }
  });
}

export function signature(value: unknown): string {
  return JSON.stringify(value) ?? "";
}
