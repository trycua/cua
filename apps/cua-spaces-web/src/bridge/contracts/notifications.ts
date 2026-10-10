// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Notifications, mirrored from the app core's `notifications.rs`
 * (libs/cua/crates/cua-spaces-app-core): the daemon's feed
 * (`notifications_list`) in, what to post and the list out.
 */

/** One feed entry (`notifications::NotificationInput`). */
export interface NotificationInput {
  id: string;
  /** Unix ms. */
  atMs: number;
  agent?: string | null;
  /** `turn_ended`, `message`, `approval`, `error`. */
  kind: string;
  title: string;
  body: string;
  read?: boolean;
}

export interface SystemNote {
  id: string;
  title: string;
  body: string;
}

/** What to post now, and the marker to keep (`notifications::NotificationsPlan`). */
export interface NotificationsPlan {
  post: SystemNote[];
  seenMs: number;
}

/** One line (`persistent::LineView`). */
export interface NotificationLine {
  id: string;
  text: string;
  trailing: string;
  actionLabel: string | null;
  secondaryLabel: string | null;
  /** Unread. */
  on: boolean | null;
}

/** `notifications::NotificationsView`. */
export interface NotificationsView {
  title: string;
  rows: NotificationLine[];
  emptyText: string;
  unread: number;
  markAllLabel: string | null;
}
