// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { HashIcon, UserRoundIcon, UsersIcon } from "lucide-react";
import { useMemo, useState } from "react";

import { useDevices, useSession, type AccessLine, type MachinesAccountView, type PendingMachine, type ThisMachineTrust } from "@/bridge";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { toast } from "@/components/ui/toast";
import { useT } from "@/i18n";
import { dateText, relativeText } from "@/lib/devices";
import type { SpaceOs } from "@/bridge";

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The account's part of your computers: what waits, this computer's trust, who reached each one. */
export function useMachinesAccount(): {
  account: MachinesAccountView | null;
  /** Computers waiting to be added (the ones put off are left out). */
  pending: PendingMachine[];
  now: number;
  devices: ReturnType<typeof useDevices>;
} {
  const { data: session } = useSession();
  const devices = useDevices();
  const account = session?.signedIn === false ? null : (devices.data?.account ?? null);
  const dismissed = devices.data?.dismissed;
  const pending = useMemo(() => (account?.pending ?? []).filter((p) => p.kind !== "sign-in" || !dismissed?.includes(p.device ?? "")), [account, dismissed]);
  return { account, pending, now: devices.data?.now ?? Math.floor(Date.now() / 1000), devices };
}

/** `macOS` as the three systems the app draws. */
export function pendingOs(p: Pick<PendingMachine, "os">): SpaceOs | null {
  const os = p.os.toLowerCase();
  return os === "macos" ? "macos" : os === "windows" ? "windows" : os === "linux" ? "linux" : null;
}

/** [Add] [Not me] for one waiting computer: the one place that does either. */
export function usePendingActions() {
  const t = useT();
  const { devices } = useMachinesAccount();
  const [notMe, setNotMe] = useState<PendingMachine | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const run = (p: PendingMachine, what: () => Promise<unknown>, done: string, failed: string) => {
    setBusy(p.id);
    void what()
      .then(
        () => toast(done, { type: "success" }),
        (e: unknown) => toast(failed, { description: errorText(e), type: "error" }),
      )
      .finally(() => setBusy(null));
  };
  return {
    busy,
    add: (p: PendingMachine) =>
      run(
        p,
        () => (p.kind === "machine" ? devices.confirmMachine(p.machine!) : devices.addMachine(p.device!)),
        t("computers.account.added", { name: p.name }),
        t("computers.account.add_failed", { name: p.name }),
      ),
    /** Not me: a computer that signed in is denied at once; one that registered is removed after a question. */
    notMe: (p: PendingMachine) =>
      p.kind === "machine"
        ? setNotMe(p)
        : run(
            p,
            () => devices.denyDevice(p.device!),
            t("computers.account.not_added", { name: p.name }),
            t("computers.account.refuse_failed", { name: p.name }),
          ),
    notMeDialog: (remove: (machine: string) => Promise<unknown>) => (
      <ConfirmDialog
        open={notMe !== null}
        onOpenChange={(o) => !o && setNotMe(null)}
        title={t("computers.account.not_you_title", { name: notMe?.name ?? "" })}
        description={t("computers.account.not_you_body")}
        confirmLabel={t("computers.account.remove")}
        destructive
        onConfirm={() =>
          notMe &&
          run(
            notMe,
            () => remove(notMe.machine!),
            t("computers.account.removed", { name: notMe.name }),
            t("computers.account.remove_failed", { name: notMe.name }),
          )
        }
      />
    ),
  };
}

/** This machine's own trust, in plain words, with the one action that fixes it. */
export function TrustSection({ trust, onTrust }: { trust: ThisMachineTrust; onTrust: () => void }) {
  const t = useT();
  const status = trust.at === null ? trust.status : `${trust.status} ${dateText(trust.at)}`;
  return (
    <>
      <h3 className="mt-5 mb-2 text-xs font-semibold text-muted-foreground">{t("computers.account.access_title")}</h3>
      <div className="border-y" data-machine-trust={trust.kind}>
        <div className="flex min-h-11 items-center justify-between gap-4 py-2.5">
          <span className={`text-[13px] ${trust.kind === "enrolled" ? "text-muted-foreground" : "text-warning"}`} data-machine-trust-status="">
            {status}
          </span>
          {trust.actionLabel ? (
            <Button size="sm" variant="outline" onClick={onTrust} data-machine-trust-action="">
              {trust.actionLabel}
            </Button>
          ) : null}
        </div>
        {trust.text ? (
          <p className="border-t py-2.5 text-xs text-muted-foreground" data-machine-trust-text={trust.tone ?? ""}>
            {trust.text}
          </p>
        ) : null}
      </div>
    </>
  );
}

/**
 * "See recent access": a plain text link at the foot of a machine's page that opens who reached the
 * machine, and when, in a small dialog. A failed read of that list stays on the page, since it says why
 * the list may be short.
 */
export function RecentAccess({ lines, title, empty, now, error }: { lines: AccessLine[]; title: string; empty: string; now: number; error?: string | null }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        type="button"
        data-machine-recent-link=""
        onClick={() => setOpen(true)}
        className="cursor-default py-1 text-[13px] text-muted-foreground hover:text-foreground hover:underline hover:underline-offset-2"
      >
        {t("computers.account.recent_access")}
      </button>
      {error ? (
        <p data-devices-read-error="" className="order-last w-full text-xs break-words text-destructive">
          {error}
        </p>
      ) : null}
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogPopup className="w-[min(480px,calc(100vw-2rem))]">
          <div className="px-5 pt-5 pb-4">
            <DialogTitle className="text-[15px] font-semibold">{title}</DialogTitle>
            <div className="mt-3 max-h-[50vh] overflow-y-auto border-y" data-machine-recent="">
              {lines.length === 0 ? <p className="py-3 text-[13px] text-muted-foreground">{empty}</p> : null}
              {lines.map((a, i) => (
                <div key={`${a.ts}-${i}`} data-recent-row="" className={`flex min-h-10 items-center justify-between gap-4 py-2 ${i > 0 ? "border-t" : ""}`}>
                  <span className="flex min-w-0 items-center gap-2 text-[13px]">
                    {a.notable ? <UsersIcon className="size-3.5 shrink-0 text-warning" aria-label={t("computers.account.notable")} /> : null}
                    <span className="select-text" data-recent-text="">
                      {a.text}
                    </span>
                  </span>
                  <span className="shrink-0 text-xs text-muted-foreground">{relativeText(a.ts, now)}</span>
                </div>
              ))}
            </div>
          </div>
          <div className="flex justify-end border-t bg-muted/60 px-5 py-3">
            <Button type="button" variant="outline" onClick={() => setOpen(false)}>
              {t("computers.account.close")}
            </Button>
          </div>
        </DialogPopup>
      </Dialog>
    </>
  );
}

/** "Trust this computer": sign in again, or add it from a computer you already trust with a one-time code. */
export function EnrollSheet() {
  const { data, chooseEnroll, backEnroll, closeEnroll } = useDevices();
  const v = data?.enroll?.view;
  return (
    <Dialog open={Boolean(v)} onOpenChange={(o) => !o && closeEnroll()}>
      <DialogPopup className="w-[min(440px,calc(100vw-2rem))]" data-enroll>
        {v ? (
          <>
            <div className="px-5 pt-5 pb-4">
              <DialogTitle className="text-[15px] font-semibold" data-enroll-title>
                {v.title}
              </DialogTitle>
              <p className="mt-1.5 text-[13px] text-muted-foreground">{v.lede}</p>
              {v.options.length > 0 ? (
                <div className="mt-4 flex flex-col gap-2">
                  {v.options.map((o) => (
                    <button
                      key={o.method}
                      type="button"
                      data-enroll-option={o.method}
                      onClick={() => void chooseEnroll(o.method)}
                      className="flex items-center gap-3 rounded-lg border bg-card px-3 py-2.5 text-left outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring/60"
                    >
                      {o.method === "sign-in" ? (
                        <UserRoundIcon className="size-5 text-muted-foreground" />
                      ) : (
                        <HashIcon className="size-5 text-muted-foreground" />
                      )}
                      <span className="min-w-0">
                        <span className="block text-[13px] font-medium">{o.title}</span>
                        <span className="block text-xs text-muted-foreground">{o.detail}</span>
                      </span>
                    </button>
                  ))}
                </div>
              ) : null}
              {v.code ? (
                <p className="mt-5 text-center font-mono text-[28px] font-semibold tracking-wider select-text" data-enroll-code>
                  {v.code}
                </p>
              ) : null}
              {v.codeHelp ? <p className="mt-3 text-xs text-muted-foreground">{v.codeHelp.replace(/`/g, "")}</p> : null}
              {v.status ? (
                <p className="mt-4 flex items-center gap-2 text-[13px] text-muted-foreground" data-enroll-status>
                  {v.busy ? <span className="size-3 animate-spin rounded-full border-2 border-muted-foreground/30 border-t-muted-foreground" /> : null}
                  {v.status}
                </p>
              ) : null}
              {v.error ? (
                <p className="mt-3 text-[13px] text-destructive select-text" data-enroll-error>
                  {v.error}
                </p>
              ) : null}
            </div>
            <div className="flex justify-end gap-2 border-t bg-muted/60 px-5 py-3">
              {v.backLabel ? (
                <Button variant="outline" onClick={backEnroll} data-enroll-back>
                  {v.backLabel}
                </Button>
              ) : null}
              <Button variant={v.done ? "default" : "outline"} onClick={closeEnroll} data-enroll-close>
                {v.closeLabel}
              </Button>
            </div>
          </>
        ) : null}
      </DialogPopup>
    </Dialog>
  );
}
