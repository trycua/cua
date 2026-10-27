// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";
import { BellIcon } from "lucide-react";
import { useState } from "react";

import { useNotifications, type NotificationInput, type NotificationLine } from "@/bridge";
import { CuaKey } from "@/components/cua-key";
import { EmptyState, Page, PageHeader } from "@/legacy/components/page";
import { Button } from "@/components/ui/button";
import { toastError } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

/** Public's page before the redesign (`spaces_redesign` off). */
export const LegacyNotificationsPage = NotificationsPage;

/**
 * Notifications: the daemon's feed, one line each, newest first, unread in
 * bold, and Mark all read while any is unread. Each line is the app core's
 * `notifications::view`, as in the SwiftUI app's `NotificationsPageView`.
 * Clicking a line opens it in place (the whole title and body, and Open
 * Space when it is about one) and marks it read; clicking again closes it.
 */
function NotificationsPage() {
  const { data, unsupported, error, markAllRead, markRead } = useNotifications();
  const view = data?.view;
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set());
  const toggle = (row: NotificationLine) => {
    const opening = !open.has(row.id);
    setOpen((was) => {
      const next = new Set(was);
      if (opening) next.add(row.id);
      else next.delete(row.id);
      return next;
    });
    // Reading it is the point; a host that can't mark one read just leaves it bold.
    if (opening && row.on) void markRead(row.id).catch(() => undefined);
  };
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
            <NotificationRow
              key={row.id}
              row={row}
              entry={data?.feed.find((n) => n.id === row.id)}
              open={open.has(row.id)}
              onToggle={() => toggle(row)}
            />
          ))}
        </div>
      )}
    </Page>
  );
}

function NotificationRow({ row, entry, open, onToggle }: { row: NotificationLine; entry: NotificationInput | undefined; open: boolean; onToggle: () => void }) {
  const detailId = `notification-${row.id}`;
  const body = entry?.body.trim() ?? "";
  return (
    <div data-notification={row.id} data-unread={String(Boolean(row.on))} data-open={String(open)}>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={open ? detailId : undefined}
        onClick={onToggle}
        className="flex min-h-11 w-full cursor-default items-center gap-3 px-4 py-2.5 text-left outline-none hover:bg-accent/50 focus-visible:bg-accent/50 focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:ring-inset"
        data-notification-toggle
      >
        <span className={cn("size-1.5 shrink-0 rounded-full", row.on ? "bg-brand" : "bg-transparent")} aria-hidden />
        <CuaKey size={20} />
        <span className={cn("min-w-0 flex-1 text-[13px]", open ? "break-words" : "truncate", row.on && "font-semibold")} data-notification-text>
          {open ? (entry?.title ?? row.text) : row.text}
        </span>
        <span className={cn("shrink-0 text-xs text-muted-foreground tabular-nums", open && "self-start pt-0.5")} data-notification-time>
          {row.trailing}
        </span>
      </button>
      {open ? (
        <div id={detailId} className="space-y-2.5 pr-4 pb-3 pl-[4.125rem]" data-notification-detail>
          {entry && body ? (
            <p className="whitespace-pre-wrap break-words text-[13px] text-muted-foreground" data-notification-body>
              {body}
            </p>
          ) : null}
          {entry?.space ? (
            <Button variant="outline" size="sm" render={<Link to="/spaces/$spaceId" params={{ spaceId: entry.space }} />} data-notification-open-space>
              Open Space
            </Button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
