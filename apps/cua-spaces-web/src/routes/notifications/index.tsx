// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, Link } from "@tanstack/react-router";
import { BellIcon, XIcon } from "lucide-react";
import { useMemo, useState } from "react";

import { useNotifications, type NotificationInput, type NotificationLine } from "@/bridge";
import { CuaKey } from "@/components/cua-key";
import { EmptyState, Page, PageHeader } from "@/components/page";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { PlainList, plainRowClass } from "@/components/ui/grouped";
import { toastError } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/notifications/")({ component: NotificationsPage });

/**
 * Notifications: the daemon's feed, one line each, newest first, unread in
 * bold, and Mark all read while any is unread. Each line is the app core's
 * `notifications::view`, as in the SwiftUI app's `NotificationsPageView`.
 * Clicking a line opens it in place (the whole title and body, and Open
 * Space when it is about one) and marks it read; clicking again closes it.
 * Each line has a delete control (shown on hover and on keyboard focus) and
 * the header has Delete all, which asks first and names the count. Both
 * delete for good, in every app that reads the same feed.
 */
function NotificationsPage() {
  const { data, unsupported, error, markAllRead, markRead, deleteOne, deleteAll } = useNotifications();
  const view = data?.view;
  const [asking, setAsking] = useState(false);
  const entries = useMemo(() => new Map(data?.feed.map((n) => [n.id, n])), [data?.feed]);
  const count = data?.feed.length ?? 0;
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
          view && view.rows.length > 0 ? (
            <>
              {view.markAllLabel ? (
                <Button variant="secondary" size="sm" onClick={() => void markAllRead().catch(toastError("Couldn't mark them read"))} data-mark-all>
                  {view.markAllLabel}
                </Button>
              ) : null}
              <Button variant="secondary" size="sm" onClick={() => setAsking(true)} data-delete-all>
                Delete all
              </Button>
            </>
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
        <PlainList data-notifications>
          {view.rows.map((row) => (
            <NotificationRow
              key={row.id}
              row={row}
              entry={entries.get(row.id)}
              open={open.has(row.id)}
              onToggle={toggle}
              onDelete={(id) => void deleteOne(id).catch(toastError("Couldn't delete the notification"))}
            />
          ))}
        </PlainList>
      )}
      <ConfirmDialog
        open={asking}
        onOpenChange={setAsking}
        title={count === 1 ? "Delete this notification?" : `Delete all ${count} notifications?`}
        description="They are removed for good, everywhere you read them."
        confirmLabel={count === 1 ? "Delete" : "Delete all"}
        destructive
        cancelFirst
        onConfirm={() => void deleteAll().catch(toastError("Couldn't delete the notifications"))}
      />
    </Page>
  );
}

function NotificationRow({
  row,
  entry,
  open,
  onToggle,
  onDelete,
}: {
  row: NotificationLine;
  entry: NotificationInput | undefined;
  open: boolean;
  onToggle: (row: NotificationLine) => void;
  onDelete: (id: string) => void;
}) {
  const detailId = `notification-${row.id}`;
  const body = entry?.body.trim() ?? "";
  return (
    <div className="group/row relative" data-notification={row.id} data-unread={String(Boolean(row.on))} data-open={String(open)}>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={open ? detailId : undefined}
        onClick={() => onToggle(row)}
        className={plainRowClass(open, "min-h-11 gap-3 px-3 py-2.5")}
        data-notification-toggle
      >
        <span className={cn("size-1.5 shrink-0 rounded-full", row.on ? "bg-brand" : "bg-transparent")} aria-hidden />
        <CuaKey size={20} />
        <span className={cn("min-w-0 flex-1 text-[13px]", open ? "break-words" : "truncate", row.on && "font-semibold")} data-notification-text>
          {open ? (entry?.title ?? row.text) : row.text}
        </span>
        <span
          className={cn(
            "shrink-0 text-xs text-muted-foreground tabular-nums group-focus-within/row:invisible group-hover/row:invisible",
            open && "self-start pt-0.5",
          )}
          data-notification-time
        >
          {row.trailing}
        </span>
      </button>
      <button
        type="button"
        aria-label="Delete notification"
        title="Delete notification"
        onClick={() => onDelete(row.id)}
        className="absolute top-2.5 right-3 inline-flex size-6 cursor-default items-center justify-center rounded-lg text-muted-foreground opacity-0 outline-none group-focus-within/row:opacity-100 group-hover/row:opacity-100 hover:bg-foreground/[0.08] hover:text-foreground focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-ring/60"
        data-notification-delete
      >
        <XIcon className="size-3.5" />
      </button>
      {open ? (
        <div id={detailId} className="space-y-2.5 pr-4 pb-3 pl-[3.625rem]" data-notification-detail>
          {entry && body ? (
            <p className="whitespace-pre-wrap break-words text-[13px] text-muted-foreground" data-notification-body>
              {body}
            </p>
          ) : null}
          {entry?.space ? (
            <Button variant="secondary" size="sm" render={<Link to="/spaces/$spaceId" params={{ spaceId: entry.space }} />} data-notification-open-space>
              Open Space
            </Button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
