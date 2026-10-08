// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Notifications (the SwiftUI app's PersistentModel feed): the daemon's
// `notifications_list`, the system notifications the core says to post for
// entries not seen yet (the marker lives in the app settings, so a restart
// never posts one again), Mark all read, and the Cua Volume's sync line the
// menu bar item shows. Polled while the app runs.
import type { AppDriveSyncInput, AppNotificationInput, AppSystemNote } from "../native/generated/index";
import type { Native } from "../native/load";

type Tool = (name: string, args?: Record<string, unknown>) => Promise<unknown>;

const rows = (v: unknown, key: string): Record<string, unknown>[] => {
  const list = v && typeof v === "object" ? (v as Record<string, unknown>)[key] : null;
  return Array.isArray(list) ? list.filter((x): x is Record<string, unknown> => !!x && typeof x === "object") : [];
};

export class NotificationsModel {
  feed: AppNotificationInput[] = [];
  /** Cua Volume's sync status (the menu's sync line), when read. */
  driveSync: AppDriveSyncInput | null = null;
  /** `volume_storage`'s backend (the menu hides sync on this machine's store with no other device). */
  driveBackend: string | null = null;
  /** Posts one system notification. */
  post: ((note: AppSystemNote) => void) | null = null;
  /** Told when the feed or the sync line changed. */
  onChange: (() => void) | null = null;

  constructor(
    private readonly native: Native,
    /** The daemon's tools; null while there is no daemon. */
    private readonly tool: () => Tool | null,
    /** The last-seen marker (the app settings file). */
    private readonly seen: { get(): bigint; set(ms: bigint): void },
  ) {}

  private call(name: string, args: Record<string, unknown> = {}): Promise<unknown> {
    const tool = this.tool();
    if (!tool) return Promise.reject(new Error("Notifications need the cua daemon"));
    return tool(name, args);
  }

  /** One poll of the feed: post what the core says, save the marker. */
  async poll(): Promise<void> {
    let r: unknown;
    try {
      r = await this.call("notifications_list");
    } catch {
      return;
    }
    const list = rows(r, "notifications").map((n) => ({
      id: n.id ?? "",
      atMs: n.at_ms ?? 0,
      agent: n.agent ?? null,
      kind: n.kind ?? "",
      title: n.title ?? "",
      body: n.body ?? "",
      read: n.read ?? false,
    }));
    let typed: AppNotificationInput[];
    try {
      typed = this.native.appNotificationsFromJson(JSON.stringify(list));
    } catch {
      return;
    }
    this.feed = typed;
    const before = this.seen.get();
    const plan = this.native.appNotificationsPlan(typed, before);
    for (const note of plan.post) this.post?.(note);
    if (plan.seenMs !== before) this.seen.set(plan.seenMs);
    this.onChange?.();
  }

  async markAllRead(): Promise<void> {
    await this.call("notifications_ack", { ids: [] }).catch(() => null);
    await this.poll();
  }

  /** The Volume's backend and sync status (the menu bar item polls them). */
  async refreshSync(): Promise<void> {
    const storage = await this.call("volume_storage").catch(() => null);
    const backend = storage && typeof storage === "object" ? (storage as Record<string, unknown>).backend : null;
    this.driveBackend = typeof backend === "string" ? backend : null;
    const sync = await this.call("volume_sync_status").catch(() => null);
    let next: AppDriveSyncInput | null = null;
    try {
      next = sync ? this.native.appDriveSyncFromJson(JSON.stringify(sync)) : null;
    } catch {
      next = null;
    }
    this.driveSync = next;
    this.onChange?.();
  }

  /** Polls every `ms` while the app runs; returns the stop. */
  startPolling(ms = 5000): () => void {
    let stopped = false;
    void (async () => {
      while (!stopped) {
        await this.poll();
        await this.refreshSync();
        await new Promise((r) => setTimeout(r, ms).unref?.());
      }
    })();
    return () => {
      stopped = true;
    };
  }
}
