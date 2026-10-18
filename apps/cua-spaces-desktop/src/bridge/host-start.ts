// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// What runs while the app runs, beyond answering the page (the SwiftUI
// app's AppDelegate.start and AppEnvironment.attach): devices asking to
// join are announced (a system notification, once per launch), the
// account's devices are read every minute, the daemon's notifications are
// polled and posted, launch at login is applied for an install that never
// chose, Local Network access is asked for on macOS, and the launch knows
// whether the keychain prompt is on screen. (The refresh after an update is
// `refreshAfterUpdate`, started in main.ts.)
// Also the menu bar item's and the tray's menu (`menuItems`).
import type { AppMenuItem, AppSpace } from "../native/generated/index";
import { LocalNetworkPermission } from "../model/local-network";
import { SecurityAgentWatcher } from "../model/security-agent";
import type { BridgeContext } from "./context";
import { hostParts } from "./host-parts";

/**
 * A machine's own desktop, not a Space: This machine, or one of your
 * machines sharing its desktop over the relay (`relay:<machine>` with no
 * host above it). The page lists them on Machines, not with the Spaces
 * (`isMachineDesktop` in the web UI), so the menu's count leaves them out
 * too (it said "4 Spaces" with 2 Spaces).
 */
export function isMachineDesktop(space: Pick<AppSpace, "id" | "status" | "provider" | "host">): boolean {
  if (space.status === "local" || space.id === "this-mac") return true;
  if (space.provider !== "relay" || space.host || !space.id.startsWith("relay:")) return false;
  const own = space.id.slice("relay:".length);
  return own.length > 0 && !own.includes("/") && !own.startsWith("space-");
}

/** The Spaces the user created (what the menu and the tray count and list). */
export const realSpaces = <T extends Pick<AppSpace, "id" | "status" | "provider" | "host">>(spaces: readonly T[]): T[] =>
  spaces.filter((s) => !isMachineDesktop(s));

/** The menu bar item's menu (the core's `appMenu`): the count, the Volume's sync line and conflicts, then Open, New Space, Settings and Quit. */
export function menuItems(ctx: BridgeContext): AppMenuItem[] {
  const { model } = ctx;
  const n = hostParts(ctx).notifications;
  const items = model.native.appMenu({
    spaces: realSpaces(model.spaces),
    keyvault: undefined,
    sync: n.driveSync ?? undefined,
    nowMs: BigInt(Date.now()),
    backend: n.driveBackend ?? undefined,
    experiments: model.settings.experiments,
  });
  // Still launching (opened at login, waiting for the keychain): say so first.
  const title = model.startup.copy.title;
  if (model.startup.isReady || !title) return items;
  const S = model.native.AppMenuItemId;
  return [{ id: S.Status, label: title, shortcut: undefined, enabled: false }, { id: S.Separator, label: "", shortcut: undefined, enabled: false }, ...items];
}

export interface HostStart {
  /** Called when the menu's input changed (the Spaces, the sync line, the launch). */
  onMenuChange(listener: () => void): () => void;
  stop(): void;
}

/** Starts the background work. */
export function startHost(ctx: BridgeContext, o: { onboarded: () => boolean }): HostStart {
  const { model } = ctx;
  const parts = hostParts(ctx);
  const stops: (() => void)[] = [];
  const menuListeners = new Set<() => void>();
  const menuChanged = () => menuListeners.forEach((l) => l());

  // (A device asking to join is announced from `hostParts`, even with the window closed.)
  // macOS asks for Local Network access once, while someone is here.
  model.host.localNetwork = new LocalNetworkPermission(ctx.platform);

  // macOS: whether the keychain prompt the launch waits on is still on screen.
  if (ctx.platform === "darwin") {
    const watcher = new SecurityAgentWatcher();
    model.startup.promptShowing = () => watcher.current;
    const follow = () => watcher.follow(model.startup.phase.kind === "waitingForKeychain");
    stops.push(model.startup.subscribe(follow), () => watcher.follow(false));
    follow();
  }

  parts.notifications.onChange = menuChanged;
  stops.push(model.subscribe((c) => c === "spaces" && menuChanged()));
  stops.push(model.startup.subscribe(menuChanged));

  let stopped = false;
  const whenReady = (f: () => void) => {
    if (model.startup.isReady) f();
    else model.startup.onReady.push(f);
  };
  whenReady(() => {
    if (stopped) return;
    stops.push(parts.notifications.startPolling());
    void (async () => {
      // What the rule reads: whether this machine provides Spaces.
      await model.host.refresh();
      await parts.loginItem.applyAtLaunch(o.onboarded());
    })();
    // The account's devices, every minute (a device asking to join is announced).
    const tick = setInterval(() => void model.devices.refresh(), 60_000);
    tick.unref?.();
    stops.push(() => clearInterval(tick));
    void model.devices.refresh();
  });

  return {
    onMenuChange(listener) {
      menuListeners.add(listener);
      return () => menuListeners.delete(listener);
    },
    stop() {
      stopped = true;
      stops.splice(0).forEach((s) => s());
    },
  };
}
