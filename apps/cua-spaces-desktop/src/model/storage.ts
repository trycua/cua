// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings → Storage (the SwiftUI app's StorageModel.swift): where the Cua
// Volume keeps its files, its mount and the block cache, as the daemon's
// tools answer them (`volume_storage`, `volume_mount_status`,
// `volume_cache_stats`), and the core's section over them
// (`appStorageSection`). A bucket an agent connected is adopted when it
// shows up. S3 keys are only ever in a `volume_storage_set` call the page
// makes; nothing here keeps them.
import * as os from "node:os";
import type { AppSettingsSection, AppStorageAction, AppStorageInput, AppStorageState } from "../native/generated/index";
import type { Native } from "../native/load";
import { words } from "./errors";

/** The page's word for the system (`StorageInput.os`). */
export function osWord(platform: NodeJS.Platform): "macos" | "windows" | "linux" {
  if (platform === "win32") return "windows";
  return platform === "linux" ? "linux" : "macos";
}

/** The user's home folder (`$HOME` first, as the daemon sees it). */
export function userHome(env: NodeJS.ProcessEnv = process.env): string {
  return env.HOME || env.USERPROFILE || os.homedir();
}

/** `~/x` under the home folder; anything else unchanged. */
export function homeExpanded(p: string, home = userHome()): string {
  if (p === "~") return home;
  if (p.startsWith("~/") || p.startsWith("~\\")) return home + p.slice(1);
  return p;
}

type Tool = (name: string, args?: Record<string, unknown>) => Promise<unknown>;

export class StorageModel {
  /** The tools' answers, as they came (null: not read, or no answer). */
  storage: unknown = null;
  mount: unknown = null;
  cache: unknown = null;
  state: AppStorageState;
  /** Where usage events go. */
  telemetry: { record(signals: import("../native/generated/index").AppTelemetrySignal[]): void } | null = null;

  constructor(
    private readonly native: Native,
    private readonly tool: Tool | null,
    private readonly platform: NodeJS.Platform = process.platform,
    private readonly home: string = userHome(),
  ) {
    this.state = native.appStorageInitial();
  }

  /** What the page and the core read: the system, the home folder and the three answers. */
  get answers(): { os: string; home: string; storage: unknown; mount: unknown; cache: unknown } {
    return { os: osWord(this.platform), home: this.home, storage: this.storage, mount: this.mount, cache: this.cache };
  }

  get input(): AppStorageInput {
    try {
      return this.native.appStorageInputFromJson(JSON.stringify(this.answers));
    } catch {
      return this.native.appStorageInputFromJson(JSON.stringify({ os: osWord(this.platform) }));
    }
  }

  get section(): AppSettingsSection {
    return this.native.appStorageSection(this.input, this.state);
  }

  private reduce(action: AppStorageAction): void {
    this.telemetry?.record(this.native.appTelemetryStorage(this.input, this.state, action));
    this.state = this.native.appStorageReduce(this.state, action);
  }

  /** Reads the three answers, each on its own (a daemon without the drive's tools leaves them null and the section says so). */
  async load(): Promise<void> {
    const read = (name: string) => (this.tool ? this.tool(name).catch(() => null) : Promise.resolve(null));
    [this.storage, this.mount, this.cache] = await Promise.all([read("volume_storage"), read("volume_mount_status"), read("volume_cache_stats")]);
    if (this.storage === null) return;
    let loaded: AppStorageAction;
    try {
      loaded = this.native.appStorageActionFromJson(JSON.stringify({ type: "loaded", storage: this.storage }));
    } catch {
      return;
    }
    const wasBusy = this.state.busy;
    this.reduce(loaded);
    // A bucket the agent connected: adopt it now.
    if (!wasBusy && this.state.request?.tag === "Adopt") await this.adopt();
  }

  private async adopt(): Promise<void> {
    const request = this.state.request;
    if (!request || request.tag !== "Adopt" || !this.tool) return;
    try {
      const update = (request as unknown as { inner: { update: Parameters<Native["appStorageUpdateJson"]>[0] } }).inner.update;
      const check = await this.tool("volume_storage_set", JSON.parse(this.native.appStorageUpdateJson(update)) as Record<string, unknown>);
      this.reduce(this.native.appStorageActionFromJson(JSON.stringify({ type: "adopted", check })));
    } catch (error) {
      this.reduce(
        new this.native.AppStorageAction.Adopted({
          check: { ok: false, reachable: false, authorized: false, versioning: false, detail: words(error), applied: false },
        }),
      );
    }
  }
}
