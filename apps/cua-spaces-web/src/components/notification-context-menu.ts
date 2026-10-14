// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { NotificationInput, NotificationLine } from "@/bridge";
import { SEPARATOR, type ContextMenuEntry } from "@/components/context-menu";

/** What a notification's menu runs: the row's own actions. */
export interface NotificationMenuActions {
  /** Opens or closes it in place (opening marks it read). */
  toggle(): void;
  markRead(): void;
  openSpace(space: string): void;
}

/**
 * A notification's context menu: Open (or Close), Mark as Read while it is
 * unread, Open Space when it is about one, and Copy Text. The daemon can
 * only mark an entry read, so there is no Mark as Unread or Dismiss.
 * Stable action ids: `notification.open`, `notification.close`,
 * `notification.markRead`, `notification.openSpace`, `notification.copy`.
 */
export function notificationMenuEntries(
  row: NotificationLine,
  entry: NotificationInput | undefined,
  open: boolean,
  act: NotificationMenuActions,
): ContextMenuEntry[] {
  const text = entry ? [entry.title, entry.body.trim()].filter(Boolean).join("\n\n") : row.text;
  return [
    open ? { id: "notification.close", label: "Close", run: act.toggle } : { id: "notification.open", label: "Open", run: act.toggle },
    row.on && { id: "notification.markRead", label: "Mark as Read", run: act.markRead },
    entry?.space && { id: "notification.openSpace", label: "Open Space", run: () => act.openSpace(entry.space!) },
    SEPARATOR,
    { id: "notification.copy", label: "Copy Text", copy: text },
  ];
}
