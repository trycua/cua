// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";
import { BellIcon } from "lucide-react";

import { useNotifications } from "@/bridge";
import { EmptyState, Page, PageHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { toastError } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/notifications/")({ component: NotificationsPage });

/**
 * Notifications: the daemon's feed, one line each, newest first, unread in
 * bold, and Mark all read while any is unread. Everything shown is the app
 * core's `notifications::view`, as in the SwiftUI app's `NotificationsPageView`.
 */
function NotificationsPage() {
  const { data, unsupported, error, markAllRead } = useNotifications();
  const view = data?.view;
  return (
    <Page className="max-w-3xl">
      <PageHeader
        title={view?.title ?? "Notifications"}
        actions={
          view?.markAllLabel ? (
            <Button variant="outline" size="sm" onClick={() => void markAllRead().catch(toastError("Couldn't mark them read"))} data-mark-all>
              {view.markAllLabel}
            </Button>
          ) : null
        }
      />
      {unsupported ? (
        <EmptyState icon={<BellIcon />} title="Notifications">
          Notifications are not available in this app yet.
        </EmptyState>
      ) : error && !data ? (
        <EmptyState icon={<BellIcon />} title="Couldn't read notifications">
          {error.message}
        </EmptyState>
      ) : !view ? null : view.rows.length === 0 ? (
        <EmptyState icon={<BellIcon />} title={view.emptyText} />
      ) : (
        <div className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs" data-notifications>
          {view.rows.map((row) => (
            <div key={row.id} data-notification={row.id} data-unread={String(Boolean(row.on))} className="flex min-h-11 items-center gap-3 px-4 py-2.5">
              <span className={cn("size-1.5 shrink-0 rounded-full", row.on ? "bg-brand" : "bg-transparent")} aria-hidden />
              <span className={cn("min-w-0 flex-1 truncate text-[13px]", row.on && "font-semibold")} title={row.text} data-notification-text>
                {row.text}
              </span>
              <span className="shrink-0 text-xs text-muted-foreground tabular-nums" data-notification-time>
                {row.trailing}
              </span>
            </div>
          ))}
        </div>
      )}
    </Page>
  );
}
