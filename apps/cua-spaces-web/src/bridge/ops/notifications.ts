// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Notifications: the daemon's feed (`notifications_list`) and marking it
 * read (`notifications_ack`), the same Spaces tools the SwiftUI app's
 * `PersistentModel` polls. `protocol.ts` registers them.
 */

import type { NotificationInput } from "../contracts/notifications";

type Empty = Record<string, never>;

export interface NotificationsOperations {
  /** The feed, as the daemon keeps it. */
  "notifications.list": { args: Empty; result: NotificationInput[] };
  /** Marks every entry read (`notifications_ack` with no ids). */
  "notifications.markAllRead": { args: Empty; result: null };
  /** Marks these entries read (`notifications_ack` with their ids). */
  "notifications.markRead": { args: { ids: string[] }; result: null };
}

export type NotificationsOpName = keyof NotificationsOperations;

export const NOTIFICATIONS_OPERATIONS = ["notifications.list", "notifications.markAllRead", "notifications.markRead"] as const satisfies readonly NotificationsOpName[];

type Handlers = { [K in NotificationsOpName]: (args: NotificationsOperations[K]["args"]) => Promise<NotificationsOperations[K]["result"]> };
type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

/** `notifications_list`'s snake_case entries, as the core reads them (`PersistentModel.pollNotifications`). */
export function notificationsFromTool(answer: unknown): NotificationInput[] {
  const rows = (answer as { notifications?: unknown[] } | null)?.notifications ?? [];
  return rows.map((raw) => {
    const n = raw as Record<string, unknown>;
    return {
      id: String(n.id ?? ""),
      atMs: typeof n.at_ms === "number" ? n.at_ms : 0,
      agent: typeof n.agent === "string" ? n.agent : null,
      kind: String(n.kind ?? ""),
      title: String(n.title ?? ""),
      body: String(n.body ?? ""),
      read: Boolean(n.read),
      space: typeof n.space === "string" && n.space ? n.space : null,
    };
  });
}

/** Tauri: both tools through `agents_tool` (already on its allow-list). */
export function tauriNotificationsOps(invoke: Invoke): Handlers {
  return {
    "notifications.list": async () => notificationsFromTool(await invoke("agents_tool", { tool: "notifications_list", args: {} })),
    "notifications.markAllRead": async () => {
      await invoke("agents_tool", { tool: "notifications_ack", args: { ids: [] } });
      return null;
    },
    "notifications.markRead": async ({ ids }) => {
      // An empty list would mark every entry read.
      if (ids.length) await invoke("agents_tool", { tool: "notifications_ack", args: { ids } });
      return null;
    },
  };
}

export const NOTIFICATIONS_TAURI_COMMANDS = {
  "notifications.list": ["agents_tool"],
  "notifications.markAllRead": ["agents_tool"],
  "notifications.markRead": ["agents_tool"],
} as const satisfies Record<NotificationsOpName, readonly string[]>;
