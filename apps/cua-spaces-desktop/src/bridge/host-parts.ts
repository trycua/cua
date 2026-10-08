// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The SwiftUI app's models behind Settings, About, launch at login, Storage,
// Notifications and the first run, made once per app model and shared by
// the areas that answer for them (about, settings, storage, notifications,
// onboarding) and by the menu bar item and tray. The system's side (the
// login item, the updater, notifications, the owner check) comes from
// `SystemServices`; without one (tests that do not set it) each is absent,
// as in a build without it.
import * as path from "node:path";
import type { AppModel } from "../model/app-model";
import { LoginItemModel } from "../model/login-item";
import { NotificationsModel } from "../model/notifications";
import { OnboardingStore } from "../model/onboarding";
import { StorageModel, userHome } from "../model/storage";
import { osLine, UpdatesModel } from "../model/updates";
import type { BridgeContext } from "./context";
import { Failure } from "./host";
import type { SystemServices } from "./system";

export interface HostParts {
  system: SystemServices | null;
  updates: UpdatesModel;
  loginItem: LoginItemModel;
  storage: StorageModel;
  notifications: NotificationsModel;
  onboarding: OnboardingStore;
  /** The daemon's tools once the live services are in (null before). */
  tool(): ((name: string, args?: Record<string, unknown>) => Promise<unknown>) | null;
}

const parts = new WeakMap<AppModel, HostParts>();

/** macOS's own version (Electron's `process.getSystemVersion`, "26.0.1"); undefined elsewhere or outside Electron. */
function systemVersion(platform: NodeJS.Platform): string | undefined {
  const get = (process as { getSystemVersion?: () => string }).getSystemVersion;
  return platform === "darwin" && typeof get === "function" ? get() : undefined;
}

/** A daemon tool, or `unsupported` with the Swift app's words when there is no daemon yet. */
export function daemonTool(ctx: BridgeContext, what = "This"): (name: string, args?: Record<string, unknown>) => Promise<unknown> {
  const tool = hostParts(ctx).tool();
  if (!tool) throw Failure.unsupported(`${what} needs the cua daemon`);
  return tool;
}

/** Persistent agents run on this machine (launch at login turns on for them), read again (a failed read keeps the last). */
async function runsAgents(model: AppModel): Promise<boolean> {
  if (model.servicesIn) await model.agents.persistent.load();
  return model.agents.persistent.count > 0;
}

export function hostParts(ctx: BridgeContext): HostParts {
  const known = parts.get(ctx.model);
  if (known) return known;
  const { model, platform } = ctx;
  const native = model.native;
  const system = ctx.system ?? null;
  const tool = () => (model.servicesIn ? (name: string, args: Record<string, unknown> = {}) => model.backend.tool(name, args) : null);
  const telemetry = model.telemetry;

  const updates = new UpdatesModel(
    native,
    system?.updater ?? null,
    { version: ctx.version, build: "", os: osLine(platform, undefined, undefined, systemVersion(platform)) },
    system?.updater?.channel === "beta" ? native.AppUpdateChannel.Beta : system?.updater ? native.AppUpdateChannel.Stable : model.settings.updateChannel,
    (channel) => {
      model.settings = { ...model.settings, updateChannel: channel };
      model.saveSettings();
    },
    platform,
  );
  updates.telemetry = telemetry;

  const loginItem = new LoginItemModel(
    native,
    system?.loginItem ?? null,
    {
      get: () => model.settings.launchAtLogin,
      set: (on) => {
        model.settings = { ...model.settings, launchAtLogin: on };
        model.saveSettings();
      },
    },
    { providesSpaces: () => Boolean(model.host.state?.configured && model.host.state.provideSpaces), runsAgents: () => runsAgents(model) },
    (feature) => model.recordFeature(feature),
  );

  const storage = new StorageModel(native, (name, args) => {
    const t = tool();
    return t ? t(name, args) : Promise.reject(new Error("No cua daemon"));
  }, platform, system?.home ?? userHome(ctx.env));
  storage.telemetry = telemetry;

  const notifications = new NotificationsModel(native, tool, {
    get: () => model.settings.notificationsSeenMs,
    set: (ms) => {
      model.settings = { ...model.settings, notificationsSeenMs: ms };
      model.saveSettings();
    },
  });
  notifications.post = (note) => system?.notify({ id: `agent-${note.id}`, title: note.title, body: note.body, route: "/notifications" });

  // Approving a device asks the person here first; a device asking to join is announced.
  if (system) {
    model.devices.presence = (reason) => system.confirmPresence(reason);
    model.devices.notify = (prompt) =>
      system.notify({ id: `device-${prompt.deviceId}`, title: prompt.notifyTitle, body: prompt.notifyBody, route: "/settings/devices" });
  }

  const onboarding = new OnboardingStore(path.join(path.dirname(model.settingsPath), "onboarding.json"));
  // Finished or shown again: the page reads the session again (`onboarding.get`).
  onboarding.subscribe(() => ctx.events.emit("session.changed"));

  const made: HostParts = { system, updates, loginItem, storage, notifications, onboarding, tool };
  parts.set(model, made);
  return made;
}
