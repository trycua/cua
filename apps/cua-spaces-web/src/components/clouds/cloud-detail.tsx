// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CircleAlertIcon, LockIcon, TriangleAlertIcon } from "lucide-react";
import { useState } from "react";

import { useCloudActions, useConnectCloud, type CloudAction, type CloudPage } from "@/bridge";
import { CloudLogo } from "@/components/cloud-logo";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { toast } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The dot beside a cloud: green when it works, amber for a warning, red when it is refused. */
export function CloudDot({ health, className }: { health: string; className?: string }) {
  const tint = health === "fail" ? "bg-destructive" : health === "warn" ? "bg-warning" : "bg-success";
  return <span aria-hidden data-cloud-health={health} className={cn("size-2 shrink-0 rounded-full", tint, className)} />;
}

/**
 * One cloud's page, in the right pane of Computers: its status, what is
 * wrong with one action each, spend against the budget, what runs there,
 * its settings and Disconnect. Hairline rows, as the machine page. Every
 * word is the app core's (`cloud_page`).
 */
export function CloudDetail({ page: p, onGone }: { page: CloudPage; onGone: () => void }) {
  const connect = useConnectCloud();
  const actions = useCloudActions();
  const [disconnecting, setDisconnecting] = useState(false);
  const [reviewing, setReviewing] = useState(false);

  const run = (a: CloudAction) => {
    if (a.id === "open" && a.url) return actions.openPage(a.url);
    if (a.id === "review") return setReviewing(true);
    // The limits change on their own sheet. Setting a cloud up again, reconnecting, switching a CLI sign-in link to
    // OIDC or pasting a new token use its connect sheet, filled from the link as it is.
    if (a.id === "raise-budget" || a.id === "change-limits") return connect.show(p.id, { limits: true });
    connect.show(p.id);
  };

  return (
    <section aria-label={`${p.name} details`} className="min-w-0 max-w-[620px]" data-cloud-detail={p.id} data-health={p.health}>
      <div className="flex items-center gap-2.5">
        <CloudLogo cloud={p.logo} size={22} />
        <h2 className="truncate text-[22px] font-semibold tracking-[-0.015em]">{p.name}</h2>
      </div>
      <p className="mt-1 flex items-center gap-1.5 text-[13px] text-muted-foreground" data-cloud-status="">
        <CloudDot health={p.health} className="size-[7px]" />
        {p.status}
      </p>

      {p.issues.length ? (
        <>
          <h3 className="mt-[22px] mb-2 text-xs font-semibold text-muted-foreground">{p.issuesTitle}</h3>
          <div className="border-y" data-cloud-issues="">
            {p.issues.map((i, n) => (
              <div key={i.id} className={cn("flex items-center gap-2.5 py-2.5", n > 0 && "border-t")} data-cloud-issue={i.id} data-level={i.level}>
                {i.level === "fail" ? (
                  <CircleAlertIcon aria-label="Problem" className="size-3.5 shrink-0 text-destructive" />
                ) : (
                  <TriangleAlertIcon aria-label="Warning" className="size-3.5 shrink-0 text-warning" />
                )}
                <span className="min-w-0 flex-1 text-[13px]">{i.text}</span>
                {i.action ? (
                  <Button size="sm" variant="outline" className="shrink-0" onClick={() => run(i.action!)} data-cloud-issue-action={i.action.id}>
                    {i.action.label}
                  </Button>
                ) : null}
              </div>
            ))}
          </div>
        </>
      ) : null}

      {p.spend ? (
        <div className="mt-[22px] border-y py-3.5" data-cloud-spend="">
          <div className="flex items-baseline justify-between gap-3">
            <p className="text-[13px] text-muted-foreground">
              <span className="text-lg font-semibold text-foreground" data-cloud-spend-amount="">
                {p.spend.amount}
              </span>{" "}
              {p.spend.ofBudget}
            </p>
            <span className="text-xs text-muted-foreground">{p.spend.resets}</span>
          </div>
          <div
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={p.spend.percent}
            className="mt-2.5 h-1 overflow-hidden rounded-full bg-muted"
          >
            <div className={cn("h-full rounded-full", p.spend.tone === "warn" ? "bg-warning" : "bg-brand")} style={{ width: `${p.spend.percent}%` }} />
          </div>
          <p className="mt-2 text-xs text-muted-foreground">{p.spend.note}</p>
        </div>
      ) : null}

      {p.running ? (
        <>
          <h3 className="mt-7 mb-2 text-xs font-semibold text-muted-foreground">{p.running.title}</h3>
          <div className="border-y" data-cloud-running="">
            {p.running.machines.map((m, i) => (
              <div key={m.id} className={cn("flex items-center gap-2.5 py-2.5 text-[13px]", i > 0 && "border-t")} data-cloud-machine={m.id}>
                <span className="truncate font-mono text-xs">{m.name}</span>
                {m.by ? <span className="min-w-0 flex-1 truncate text-muted-foreground">{m.by}</span> : <span className="flex-1" />}
                {m.cost ? <span className="shrink-0 text-xs text-muted-foreground">{m.cost}</span> : null}
              </div>
            ))}
          </div>
        </>
      ) : null}

      {p.token ? (
        <>
          <h3 className="mt-7 mb-2 text-xs font-semibold text-muted-foreground">{p.token.title}</h3>
          <div className="border-y" data-cloud-token="">
            <div className="py-2.5">
              <p className="flex items-center gap-2 text-[13px]">
                <LockIcon className="size-3.5 text-muted-foreground" />
                {p.token.lock}
              </p>
              <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{p.token.note}</p>
            </div>
            {p.token.holders ? (
              <div className="border-t py-2.5">
                <p className="text-[13px]">{p.token.holdersLabel}</p>
                <p className="text-xs text-muted-foreground">{p.token.holders}</p>
              </div>
            ) : null}
            <div className="flex items-center justify-between gap-4 border-t py-2.5">
              <div>
                <p className="text-[13px]">{p.token.revokeLabel}</p>
                <p className="text-xs text-muted-foreground">{p.token.revokeHelp}</p>
              </div>
              <Button size="sm" variant="outline" className="shrink-0" onClick={() => run(p.token!.replace)} data-cloud-new-token="">
                {p.token.replace.label}
              </Button>
            </div>
          </div>
        </>
      ) : null}

      {p.settings.length ? (
        <>
          <div className="mt-7 mb-2 flex items-center justify-between">
            <h3 className="text-xs font-semibold text-muted-foreground">{p.settingsTitle}</h3>
            {p.limits ? (
              <button type="button" onClick={() => run(p.limits!)} className="cursor-default text-xs text-brand-strong hover:underline" data-cloud-change-limits="">
                {p.limits.label}
              </button>
            ) : null}
          </div>
          <dl className="border-y" data-cloud-settings="">
            {p.settings.map((s, i) => (
              <div key={s.id} className={cn("flex items-center justify-between gap-4 py-2.5", i > 0 && "border-t")} data-cloud-setting={s.id}>
                <div>
                  <dt className="text-[13px]">{s.label}</dt>
                  {s.help ? <p className="text-xs text-muted-foreground">{s.help}</p> : null}
                </div>
                {s.value ? <dd className="shrink-0 select-text text-[13px]">{s.value}</dd> : null}
                {s.id === "access" && p.switchAuth ? (
                  <dd className="shrink-0">
                    <Button size="sm" variant="outline" onClick={() => run(p.switchAuth!)} data-cloud-switch={p.switchAuth.id}>
                      {p.switchAuth.label}
                    </Button>
                  </dd>
                ) : null}
              </div>
            ))}
          </dl>
        </>
      ) : null}

      {p.disconnect ? (
        <button
          type="button"
          onClick={() => setDisconnecting(true)}
          className="mt-7 block cursor-default py-1 text-[13px] text-destructive hover:underline hover:underline-offset-2"
          data-cloud-disconnect=""
        >
          {p.disconnect.label}
        </button>
      ) : null}

      {p.disconnect ? (
        <ConfirmDialog
          open={disconnecting}
          onOpenChange={setDisconnecting}
          title={p.disconnect.confirm.title}
          description={p.disconnect.confirm.message}
          confirmLabel={p.disconnect.confirm.confirmLabel}
          cancelLabel={p.disconnect.confirm.cancelLabel}
          destructive
          onConfirm={() =>
            void actions.disconnect(p.id, p.disconnect!.delete).then(
              () => {
                onGone();
                toast(`Disconnected ${p.name}`, { type: "success" });
              },
              (e: unknown) => toast(`Couldn't disconnect ${p.name}`, { description: errorText(e), type: "error" }),
            )
          }
        />
      ) : null}

      {p.approval ? <ApprovalDialog open={reviewing} onOpenChange={setReviewing} page={p} /> : null}
    </section>
  );
}

/** "Approve AWS for your agent?": what the agent set up, and Approve or Deny. */
function ApprovalDialog({ open, onOpenChange, page: p }: { open: boolean; onOpenChange: (open: boolean) => void; page: CloudPage }) {
  const actions = useCloudActions();
  const [busy, setBusy] = useState(false);
  const a = p.approval!;
  const answer = (approve: boolean) => {
    setBusy(true);
    void actions
      .approve(p.id, approve)
      .then(
        () => {
          onOpenChange(false);
          toast(approve ? `Approved ${p.name}` : `Denied ${p.name}`, { type: "success" });
        },
        (e: unknown) => toast(`Couldn't ${approve ? "approve" : "deny"} ${p.name}`, { description: errorText(e), type: "error" }),
      )
      .finally(() => setBusy(false));
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogPopup className="top-[14vh] w-[min(440px,calc(100vw-2rem))]" style={{ backgroundColor: "var(--popover)" }} data-cloud-approval="">
        <div className="px-6 pt-5 pb-3">
          <div className="flex items-center gap-2">
            <CloudLogo cloud={p.logo} size={16} />
            <DialogTitle className="text-[15px] font-semibold">{a.title}</DialogTitle>
          </div>
          <p className="mt-1.5 text-[13px] leading-relaxed text-muted-foreground">{a.intro}</p>
          <dl className="mt-3.5 divide-y border-y">
            {a.rows.map((r) => (
              <div key={r.label} className="flex items-center justify-between gap-4 py-2.5 text-[13px]">
                <dt>{r.label}</dt>
                <dd className="text-muted-foreground">{r.value}</dd>
              </div>
            ))}
          </dl>
          <p className="mt-2.5 text-xs text-muted-foreground">{a.note}</p>
        </div>
        <div className="flex items-center justify-between border-t px-6 py-3">
          <Button variant="outline" disabled={busy} onClick={() => answer(false)} data-cloud-deny="">
            {a.denyLabel}
          </Button>
          <Button disabled={busy} onClick={() => answer(true)} data-cloud-approve="">
            {a.approveLabel}
          </Button>
        </div>
      </DialogPopup>
    </Dialog>
  );
}
