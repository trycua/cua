// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "Launch Cua Spaces at login" on each system (the SwiftUI app's
// `SMAppService.mainApp`): macOS registers the app itself as a login item
// (Electron's `mainAppService`, the same SMAppService, so it can wait for
// approval in System Settings); Windows adds it to the user's startup apps
// (Task Manager can turn it off, which reads as waiting for approval);
// Linux writes an XDG autostart entry. Each opens the app quietly: no
// window, as the Swift app starts in the menu bar at login. A development
// run has none (it would register the bare Electron binary).
import { app, shell } from "electron";
import * as os from "node:os";
import * as path from "node:path";
import { LinuxAutostart, type LoginItemControlling, type LoginItemStatus } from "./model/login-item";

/** The flag a login launch carries on Windows and Linux. */
export const HIDDEN_ARG = "--hidden";

const MAC_STATUS: Record<string, LoginItemStatus> = {
  enabled: "enabled",
  "not-registered": "notRegistered",
  "requires-approval": "requiresApproval",
  "not-found": "notFound",
};

class MacLoginItem implements LoginItemControlling {
  status(): LoginItemStatus {
    const s = app.getLoginItemSettings({ type: "mainAppService" }) as Electron.LoginItemSettings & { status?: string };
    return MAC_STATUS[s.status ?? ""] ?? (s.openAtLogin ? "enabled" : "notRegistered");
  }
  register() {
    app.setLoginItemSettings({ openAtLogin: true, type: "mainAppService" });
  }
  unregister() {
    app.setLoginItemSettings({ openAtLogin: false, type: "mainAppService" });
  }
  openSystemSettings() {
    void shell.openExternal("x-apple.systempreferences:com.apple.LoginItems-Settings.extension");
  }
}

class WindowsLoginItem implements LoginItemControlling {
  private readonly options = { path: process.execPath, args: [HIDDEN_ARG] };
  status(): LoginItemStatus {
    const s = app.getLoginItemSettings(this.options);
    if (!s.openAtLogin) return "notRegistered";
    // Turned off under Startup apps: on, but waiting for the user there.
    return s.executableWillLaunchAtLogin === false ? "requiresApproval" : "enabled";
  }
  register() {
    app.setLoginItemSettings({ ...this.options, openAtLogin: true });
  }
  unregister() {
    app.setLoginItemSettings({ ...this.options, openAtLogin: false });
  }
  openSystemSettings() {
    void shell.openExternal("ms-settings:startupapps");
  }
}

/** This build's login item; null in a development run. */
export function makeLoginItem(platform: NodeJS.Platform = process.platform): LoginItemControlling | null {
  if (!app.isPackaged) return null;
  if (platform === "darwin") return new MacLoginItem();
  if (platform === "win32") return new WindowsLoginItem();
  if (platform === "linux") {
    const config = process.env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config");
    return new LinuxAutostart(path.join(config, "autostart"), process.env.APPIMAGE || process.execPath, (dir) => void shell.openPath(dir));
  }
  return null;
}

/** Whether the system opened the app at login (macOS says so; Windows and Linux pass `--hidden`). */
export function openedAtLogin(platform: NodeJS.Platform = process.platform, argv: string[] = process.argv): boolean {
  if (platform === "darwin") {
    try {
      return app.getLoginItemSettings().wasOpenedAtLogin === true;
    } catch {
      return false;
    }
  }
  return argv.includes(HIDDEN_ARG);
}
