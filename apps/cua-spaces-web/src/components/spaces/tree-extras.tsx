// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * What sits around the list of your computers (CUA-3551), all of it the
 * core's `machines_view` and `machines_account`: computers waiting for you
 * to add them, the clouds you connected, what else is signed in to your
 * account, and removing the computers offline for 30+ days at once.
 */

import { Link } from "@tanstack/react-router";
import { ChevronRightIcon, PlusIcon } from "lucide-react";
import { useState } from "react";

import { useConnectCloud, useMachines, type PendingMachine } from "@/bridge";
import { CloudDot } from "@/components/clouds/cloud-detail";
import { CloudLogo } from "@/components/cloud-logo";
import { pendingOs, type usePendingActions } from "@/components/machines/account-parts";
import { OsIcon } from "@/components/os-icon";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { GroupedAction, GroupedRow, GroupedSection } from "@/components/ui/grouped";
import { toast } from "@/components/ui/toast";
import { useT } from "@/i18n";
import { machineOs } from "@/lib/machines";

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

const rowLink = "flex min-h-11 items-center gap-3 px-4 py-2.5 text-[13px] outline-none hover:bg-accent/50 focus-visible:bg-accent/50";

/** Computers that signed in or were set up and wait for you: Add, or Not me. */
export function PendingSection({
  pending,
  title,
  actions,
}: {
  pending: readonly PendingMachine[];
  title: string;
  actions: ReturnType<typeof usePendingActions>;
}) {
  if (pending.length === 0) return null;
  return (
    <GroupedSection title={title} className="mb-6" data-tree-pending={pending.length}>
      {pending.map((p) => {
        const os = pendingOs(p);
        const busy = actions.busy === p.id;
        return (
          <GroupedRow key={p.id} data-pending-machine={p.id} data-pending-kind={p.kind}>
            <span aria-hidden className="size-2 shrink-0 rounded-full bg-warning" />
            {os ? <OsIcon os={os} className="size-4 shrink-0 text-muted-foreground" /> : null}
            <span className="min-w-0 flex-1">
              <span className="block truncate font-medium">{p.name}</span>
              <span className="block truncate text-xs text-muted-foreground" data-pending-line="">
                {p.line}
              </span>
            </span>
            <Button size="sm" disabled={busy} onClick={() => actions.add(p)} data-pending-add="">
              {p.addLabel}
            </Button>
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => actions.notMe(p)} data-pending-not-me="">
              {p.notMeLabel}
            </Button>
          </GroupedRow>
        );
      })}
    </GroupedSection>
  );
}

/** The clouds you connected, each opening its page, and Connect a cloud. */
export function CloudsSection() {
  const { view } = useMachines();
  const connect = useConnectCloud();
  const clouds = view?.clouds;
  if (!clouds) return null;
  return (
    <GroupedSection title={clouds.group.title} className="mt-8" data-cloud-group="clouds">
      {clouds.group.rows.map((c) => (
        <Link key={c.id} to="/spaces/cloud/$cloudId" params={{ cloudId: c.id }} className={rowLink} data-cloud-id={c.id} data-health={c.health}>
          <CloudDot health={c.health} />
          <CloudLogo cloud={c.logo} size={16} />
          <span className="min-w-0 flex-1">
            <span className="block truncate font-medium">{c.name}</span>
            <span className="block truncate text-xs text-muted-foreground" data-cloud-line="">
              {c.line}
            </span>
          </span>
          <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground/60" />
        </Link>
      ))}
      <GroupedAction onClick={() => connect.show()} data-connect-a-cloud="">
        <PlusIcon className="size-3.5" />
        {clouds.group.connectLabel}
      </GroupedAction>
    </GroupedSection>
  );
}

/**
 * Whatever else is signed in to your account without sharing anything (a
 * laptop that only signed in, a browser): not a computer of the list, so
 * listed apart, each opening its page (Rename, Revoke).
 */
export function DevicesSection() {
  const t = useT();
  const { data = [], view } = useMachines();
  const signedIn = new Set(data.filter((m) => m.device && !m.current).map((m) => m.id));
  const rows = (view?.machines ?? []).filter((m) => signedIn.has(m.id));
  if (rows.length === 0) return null;
  return (
    <GroupedSection title={t("computers.tree.also_signed_in")} className="mt-8" data-tree-signed-in="">
      {rows.map((m) => {
        const os = machineOs(m);
        return (
          <Link
            key={m.id}
            to="/spaces/computer/$computerId"
            params={{ computerId: m.id }}
            className={rowLink}
            data-machine-id={m.id}
            data-machine-name={m.name}
            data-online={String(m.online)}
          >
            {os ? <OsIcon os={os} className="size-4 shrink-0 text-muted-foreground" /> : null}
            <span className="min-w-0 flex-1">
              <span className="block truncate font-medium">{m.name}</span>
              <span className="block truncate text-xs text-muted-foreground" data-machine-line="">
                {m.line}
              </span>
            </span>
            <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground/60" />
          </Link>
        );
      })}
    </GroupedSection>
  );
}

/** "Remove old computers" beside the offline row, while some were offline for 30+ days. */
export function RemoveOldButton() {
  const t = useT();
  const { view, remove } = useMachines();
  const [asking, setAsking] = useState(false);
  const removeOld = view?.groups.find((g) => g.removeAll)?.removeAll ?? null;
  if (!removeOld) return null;
  return (
    <>
      <button
        type="button"
        onClick={() => setAsking(true)}
        className="ml-auto shrink-0 cursor-default px-1 text-xs text-muted-foreground underline decoration-border underline-offset-2 hover:text-foreground"
        data-machines-remove-old=""
      >
        {removeOld.label}
      </button>
      <ConfirmDialog
        open={asking}
        onOpenChange={setAsking}
        title={removeOld.confirm.title}
        description={removeOld.confirm.message}
        confirmLabel={removeOld.confirm.confirmLabel}
        cancelLabel={removeOld.confirm.cancelLabel}
        destructive
        onConfirm={() => {
          void (async () => {
            for (const r of removeOld.removes) {
              try {
                await remove(r.machine ?? null, r.device ?? null);
              } catch (e) {
                toast(t("computers.tree.remove_old_failed"), { description: errorText(e), type: "error" });
                return;
              }
            }
            toast(t("computers.tree.removed_old", { count: removeOld.removes.length }), { type: "success" });
          })();
        }}
      />
    </>
  );
}
