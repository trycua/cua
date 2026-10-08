// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The app as a login item ("Launch Cua Spaces at login", the SwiftUI app's
// LoginItem.swift and AppModel's launch-at-login): what the system
// reports, the user's choice saved in the app settings, and the core's rule
// for an install that never chose (turned on when this machine provides
// Spaces or runs persistent agents). Everything that registers or reads it
// goes through `LoginItemControlling`, so tests never touch the real login
// items. `src/login-item.ts` is the Electron one for each system.
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import * as path from "node:path";
import type { AppLoginItemStatus } from "../native/generated/index";
import type { Native } from "../native/load";
import { words } from "./errors";

/** The status words the core reads (`AppLoginItemStatus`'s values). */
export type LoginItemStatus = "enabled" | "notRegistered" | "requiresApproval" | "notFound";

export interface LoginItemControlling {
  /** What the system reports now. */
  status(): LoginItemStatus;
  /** Registers the app (it opens at login, once allowed). */
  register(): void;
  unregister(): void;
  /** Where the system lists login items (macOS Login Items, Windows Startup apps). */
  openSystemSettings(): void;
}

/** An in-memory login item (tests). `approval`: a register waits for approval, as macOS does after the user turned the app off. */
export class FixtureLoginItem implements LoginItemControlling {
  approval = false;
  failure: string | null = null;
  readonly calls: string[] = [];
  constructor(public current: LoginItemStatus = "notRegistered") {}
  status() {
    return this.current;
  }
  register() {
    this.calls.push("register");
    if (this.failure) throw new Error(this.failure);
    this.current = this.approval ? "requiresApproval" : "enabled";
  }
  unregister() {
    this.calls.push("unregister");
    if (this.failure) throw new Error(this.failure);
    this.current = "notRegistered";
  }
  openSystemSettings() {
    this.calls.push("open-settings");
  }
}

/** What the login-item rows read (`AppLoginItemInput` less `busy`). */
export interface LoginItemReport {
  status: LoginItemStatus;
  providesSpaces: boolean;
  runsAgents: boolean;
}

/** The app's side of it: reads, the user's choice, and the launch rule. */
export class LoginItemModel {
  /** What the system reported last (null until read). */
  status: LoginItemStatus | null = null;
  busy = false;
  error: string | null = null;
  /** Whether persistent agents ran here at the last read. */
  runsAgents = false;

  constructor(
    private readonly native: Native,
    readonly item: LoginItemControlling | null,
    /** The user's saved choice (`AppSettings.launchAtLogin`), and saving it. */
    private readonly choice: { get(): boolean | undefined; set(on: boolean | undefined): void },
    /** This machine provides Spaces, and runs persistent agents. */
    private readonly serves: { providesSpaces(): boolean; runsAgents(): Promise<boolean> | boolean },
    private readonly record: (feature: string) => void = () => {},
  ) {}

  read(): LoginItemStatus | null {
    this.status = this.item?.status() ?? null;
    return this.status;
  }

  /** The rows' input; null when this build has no login item. */
  async report(): Promise<LoginItemReport | null> {
    const status = this.read();
    if (status === null) return null;
    this.runsAgents = await this.serves.runsAgents();
    return { status, providesSpaces: this.serves.providesSpaces(), runsAgents: this.runsAgents };
  }

  /** The user turned it on or off (Settings, or the first run's Done): saved as their choice, applied, and read back. */
  set(on: boolean): void {
    const item = this.item;
    if (!item || this.busy) return;
    this.choice.set(on);
    this.busy = true;
    this.error = null;
    try {
      if (on) item.register();
      else item.unregister();
      this.record(on ? "launch_at_login_on" : "launch_at_login_off");
    } catch (error) {
      this.error = words(error);
    }
    this.status = item.status();
    this.busy = false;
  }

  /**
   * At launch, once the first run is done: an install that never chose (its
   * first run predates the setting) is turned on when this machine provides
   * Spaces or runs persistent agents (the core's rule); a choice stands.
   */
  async applyAtLaunch(onboarded: boolean): Promise<void> {
    const item = this.item;
    if (!item || !onboarded || this.choice.get() !== undefined) {
      this.read();
      return;
    }
    const serves = this.serves.providesSpaces() || (await this.serves.runsAgents());
    const plan = this.native.appLoginItemLaunchPlan(undefined, true, serves, item.status() as AppLoginItemStatus);
    if (plan.register) {
      try {
        item.register();
      } catch (error) {
        console.warn(`[cua-spaces] launch at login: ${words(error)}`);
      }
    }
    if (plan.record !== undefined) this.choice.set(plan.record);
    this.read();
  }
}

/** Linux: an XDG autostart entry (`~/.config/autostart/cua-spaces.desktop`), which every desktop that autostarts reads. */
export class LinuxAutostart implements LoginItemControlling {
  constructor(
    /** The autostart folder (`$XDG_CONFIG_HOME/autostart`). */
    readonly dir: string,
    /** What starts the app (the AppImage when it runs from one). */
    private readonly exec: string,
    private readonly openFolder: (dir: string) => void = () => {},
  ) {}

  static readonly FILE = "cua-spaces.desktop";

  get file(): string {
    return path.join(this.dir, LinuxAutostart.FILE);
  }

  /** The entry: the app, quietly (`--hidden`: no window, as macOS opens a login item). */
  static entry(exec: string): string {
    const quoted = /[\s"'\\$`]/.test(exec) ? `"${exec.replace(/(["\\$`])/g, "\\$1")}"` : exec;
    return ["[Desktop Entry]", "Type=Application", "Name=Cua Spaces", `Exec=${quoted} --hidden`, "Icon=cua-spaces", "Terminal=false", "X-GNOME-Autostart-enabled=true", ""].join("\n");
  }

  /** On while the entry exists and no desktop turned it off (`Hidden=true`, `X-GNOME-Autostart-enabled=false`). */
  status(): LoginItemStatus {
    let text: string;
    try {
      text = readFileSync(this.file, "utf8");
    } catch {
      return "notRegistered";
    }
    if (/^\s*Hidden\s*=\s*true\s*$/im.test(text) || /^\s*X-GNOME-Autostart-enabled\s*=\s*false\s*$/im.test(text)) return "notRegistered";
    return "enabled";
  }

  register(): void {
    mkdirSync(this.dir, { recursive: true });
    writeFileSync(this.file, LinuxAutostart.entry(this.exec));
  }

  unregister(): void {
    rmSync(this.file, { force: true });
  }

  openSystemSettings(): void {
    this.openFolder(this.dir);
  }
}
