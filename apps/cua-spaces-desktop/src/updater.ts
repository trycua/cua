// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// electron-updater against the feed electron-builder bakes into
// app-update.yml: the rolling `cua-spaces-latest` release of the repository
// the build came from (electron-builder.config.cjs). It stands where Sparkle
// does in the Swift app (Settings → About, `model/updates.ts`): automatic
// checks, automatic installs, the channel, Check Now with its own windows
// for what it found, and the last check.
//
// Opt-in: nothing checks on its own unless the user turns automatic checks
// on (Settings → About, `"autoUpdate": true` in settings.json, or
// CUA_SPACES_AUTO_UPDATE=1). Someone who moved over from the Swift app keeps
// Sparkle's choices instead (update-prefs.ts): checks on unless they had
// turned them off there. The beta channel (prereleases, `beta*.yml`) is
// served everywhere. The stable channel (`latest*.yml`) is served on Windows
// and Linux, where stable releases ship the Electron app. On macOS stable
// releases still ship the Swift app, updated by Sparkle; Check Now on Stable
// says so until the Sparkle cutover (docs/sparkle-cutover.md) publishes
// `latest-mac.yml` and STABLE_FEED turns on.
import { app, BrowserWindow, dialog } from "electron";
import { existsSync } from "node:fs";
import * as path from "node:path";
import { electronUpdater } from "./electron-updater";
import { recordUpdateChannel } from "./mesh-channel";
import type { UpdaterDriving } from "./model/updates";
import { readSettings, writeSettings } from "./settings";
import { autoCheckPref, autoInstallPref, channelPref, type UpdateChannel } from "./update-prefs";

export type { UpdateChannel } from "./update-prefs";

const POLL_MS = 4 * 60 * 60 * 1000;

/** macOS only: off until stable releases publish the Electron macOS feed (the cutover). */
export const STABLE_FEED = false;

/** The channel a version was cut for: X.Y.Z-suffix is a beta. */
export const buildChannel = (version = app.getVersion()): UpdateChannel => (/^\d+\.\d+\.\d+-/.test(version) ? "beta" : "stable");


/** The feed file electron-updater reads: `beta*.yml` or `latest*.yml`. */
export const feedChannel = (channel: UpdateChannel) => (channel === "beta" ? "beta" : "latest");

/** Stable Electron builds exist for Windows and Linux; macOS waits for STABLE_FEED. */
export const channelServed = (
  channel: UpdateChannel,
  stableFeed = STABLE_FEED,
  platform: NodeJS.Platform = process.platform,
) => channel === "beta" || stableFeed || platform === "win32" || platform === "linux";

export const updateChannel = (): UpdateChannel => channelPref(readSettings(), buildChannel(), (c) => channelServed(c));

/** A packaged build with a feed (electron-builder's app-update.yml); a development run has no updater. */
export const hasFeed = () => app.isPackaged && existsSync(path.join(process.resourcesPath, "app-update.yml"));

export const updatesEnabled = () =>
  app.isPackaged && autoCheckPref(readSettings(), process.env) && channelServed(updateChannel());

type AutoUpdater = (typeof import("electron-updater"))["autoUpdater"];

/** The window an update dialog belongs to (the focused one, else any). */
const parent = () => BrowserWindow.getFocusedWindow() ?? BrowserWindow.getAllWindows()[0] ?? null;

async function message(options: Electron.MessageBoxOptions): Promise<number> {
  app.focus({ steal: true });
  const win = parent();
  const r = win ? await dialog.showMessageBox(win, options) : await dialog.showMessageBox(options);
  return r.response;
}

/** Settings → About's updater: electron-updater with Sparkle's choices and windows. */
export class ElectronUpdater implements UpdaterDriving {
  onChange: (() => void) | null = null;
  onUpdateEvent: ((event: string) => void) | null = null;
  private checking = false;
  /** The check the user started shows "up to date" and errors; a background one only what it found. */
  private userCheck = false;
  private poll: NodeJS.Timeout | null = null;
  private loaded: Promise<AutoUpdater> | null = null;
  private listening = false;
  /** This check's failure was reported (electron-updater both emits and rejects). */
  private reported = false;

  /** The updater for this build; null in a development run (no feed). */
  static start(): ElectronUpdater | null {
    // Where the cua daemon and this machine's host read it (mesh-channel.ts).
    if (app.isPackaged) recordUpdateChannel(updateChannel());
    if (!hasFeed()) return null;
    const u = new ElectronUpdater();
    u.schedule();
    return u;
  }

  get automaticallyChecks(): boolean {
    return autoCheckPref(readSettings(), process.env);
  }
  set automaticallyChecks(on: boolean) {
    writeSettings({ autoUpdate: on });
    this.schedule();
    this.onChange?.();
  }

  get automaticallyInstalls(): boolean {
    return autoInstallPref(readSettings());
  }
  set automaticallyInstalls(on: boolean) {
    writeSettings({ autoInstall: on });
    void this.updater().then((u) => (u.autoDownload = on));
    this.onChange?.();
  }

  get lastCheck(): number | null {
    return readSettings().lastUpdateCheck ?? null;
  }

  get canCheck(): boolean {
    return !this.checking;
  }

  get channel(): UpdateChannel {
    return updateChannel();
  }
  set channel(channel: UpdateChannel) {
    // The model sets the channel it shows at launch: only a change is the user's choice.
    if (channel === updateChannel()) return;
    writeSettings({ updateChannel: channel, updateChannelChosen: true });
    recordUpdateChannel(channel);
    this.loaded = null;
    this.schedule();
  }

  checkNow(): void {
    if (this.checking) return;
    if (!channelServed(this.channel)) {
      void message({
        type: "info",
        message: "Stable updates come with a later release",
        detail: "This version of Cua Spaces updates from the Beta channel. Choose Beta under “Update to” to get updates now.",
        buttons: ["OK"],
      });
      return;
    }
    this.userCheck = true;
    void this.check();
  }

  /** Background checks every few hours while automatic checks are on and the channel is served. */
  private schedule(): void {
    if (this.poll) clearInterval(this.poll);
    this.poll = null;
    if (!this.automaticallyChecks || !channelServed(this.channel)) return;
    void this.check();
    this.poll = setInterval(() => void this.check(), POLL_MS);
    this.poll.unref();
  }

  private updater(): Promise<AutoUpdater> {
    this.loaded ??= (async () => {
      const { autoUpdater } = await electronUpdater();
      const channel = this.channel;
      autoUpdater.channel = feedChannel(channel);
      // Betas read beta*.yml (after the cutover stable releases write it too,
      // so betas move on to the next stable); stable never sees a beta.
      autoUpdater.allowPrerelease = channel === "beta";
      autoUpdater.allowDowngrade = false;
      autoUpdater.autoDownload = this.automaticallyInstalls;
      autoUpdater.autoInstallOnAppQuit = true;
      if (!this.listening) {
        this.listening = true;
        autoUpdater.on("update-available", (i) => void this.found(i.version));
        autoUpdater.on("update-not-available", () => this.notFound());
        autoUpdater.on("update-downloaded", (i) => void this.downloaded(i.version));
        autoUpdater.on("error", (e) => this.failed(e));
      }
      return autoUpdater;
    })();
    return this.loaded;
  }

  private async check(): Promise<void> {
    if (this.checking) return;
    this.checking = true;
    this.reported = false;
    this.onChange?.();
    try {
      const u = await this.updater();
      await u.checkForUpdates();
    } catch (error) {
      if (!this.reported) this.failed(error instanceof Error ? error : new Error(String(error)));
    } finally {
      writeSettings({ lastUpdateCheck: Date.now() });
      this.checking = false;
      this.onChange?.();
    }
  }

  private async found(version: string): Promise<void> {
    this.onUpdateEvent?.("found");
    const user = this.userCheck;
    this.userCheck = false;
    if (this.automaticallyInstalls) return; // Downloading; `downloaded` asks to relaunch.
    const choice = await message({
      type: "info",
      message: "A new version of Cua Spaces is available!",
      detail: `Cua Spaces ${version} is now available—you have ${app.getVersion()}. Would you like to download it now?`,
      buttons: ["Install Update", user ? "Not Now" : "Remind Me Later"],
      defaultId: 0,
      cancelId: 1,
    });
    if (choice === 0) void (await this.updater()).downloadUpdate().catch((e: unknown) => this.failed(e instanceof Error ? e : new Error(String(e))));
  }

  private notFound(): void {
    this.onUpdateEvent?.("not_found");
    if (!this.userCheck) return;
    this.userCheck = false;
    void message({
      type: "info",
      message: "You’re up to date!",
      detail: `Cua Spaces ${app.getVersion()} is currently the newest version available.`,
      buttons: ["OK"],
    });
  }

  private async downloaded(version: string): Promise<void> {
    const choice = await message({
      type: "info",
      message: "Ready to Install",
      detail: `Cua Spaces ${version} has been downloaded and is ready to use! Would you like to install it and relaunch Cua Spaces now?`,
      buttons: ["Install and Relaunch", "Install on Quit"],
      defaultId: 0,
      cancelId: 1,
    });
    if (choice !== 0) return;
    this.onUpdateEvent?.("installed");
    (await this.updater()).quitAndInstall();
  }

  private failed(error: Error): void {
    this.reported = true;
    this.onUpdateEvent?.("failed");
    const user = this.userCheck;
    this.userCheck = false;
    console.warn(`[cua-spaces] update: ${error.message}`);
    if (!user) return;
    void message({ type: "warning", message: "Update Error!", detail: `An error occurred while checking for updates: ${error.message}`, buttons: ["OK"] });
  }
}
