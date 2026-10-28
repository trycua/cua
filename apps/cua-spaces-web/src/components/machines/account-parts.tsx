// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { HashIcon, UserRoundIcon, UsersIcon } from "lucide-react";
import { useMemo, useState } from "react";

import { useDevices, useSession, type AccessLine, type MachinesAccountView, type PendingMachine, type ThisMachineTrust } from "@/bridge";
import { OsIcon } from "@/components/os-icon";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { toast } from "@/components/ui/toast";
import { dateText, relativeText } from "@/lib/devices";
import type { SpaceOs } from "@/bridge";

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The account's part of the Machines page: what waits, this machine's trust, who reached each machine. */
export function useMachinesAccount(): {
  account: MachinesAccountView | null;
  /** Machines waiting to be added (the ones put off are left out). */
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

/** [Add machine] [Not me] for one waiting machine: the one place that does either. */
export function usePendingActions() {
  const { devices } = useMachinesAccount();
  const [notMe, setNotMe] = useState<PendingMachine | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const run = (p: PendingMachine, what: () => Promise<unknown>, done: string, failed: string) => {
    setBusy(p.id);
    void what().then(
      () => toast(done, { type: "success" }),
      (e: unknown) => toast(failed, { description: errorText(e), type: "error" }),
    ).finally(() => setBusy(null));
  };
  return {
    busy,
    add: (p: PendingMachine) =>
      run(p, () => (p.kind === "machine" ? devices.confirmMachine(p.machine!) : devices.addMachine(p.device!)), `Added ${p.name}`, `Couldn't add ${p.name}`),
    /** Not me: a machine that signed in is denied at once; one that registered is removed after a question. */
    notMe: (p: PendingMachine) => (p.kind === "machine" ? setNotMe(p) : run(p, () => devices.denyDevice(p.device!), `${p.name} was not added`, `Couldn't refuse ${p.name}`)),
    notMeDialog: (remove: (machine: string) => Promise<unknown>) => (
      <ConfirmDialog
        open={notMe !== null}
        onOpenChange={(o) => !o && setNotMe(null)}
        title={`Not you? Remove “${notMe?.name ?? ""}”`}
        description="It leaves your account. Setting it up again adds it back."
        confirmLabel="Remove"
        destructive
        onConfirm={() => notMe && run(notMe, () => remove(notMe.machine!), `${notMe.name} was removed`, `Couldn't remove ${notMe.name}`)}
      />
    ),
  };
}

/** One waiting machine in the list's top group: name, system, its line, and the two choices. */
export function PendingRow({
  machine: p,
  selected,
  busy,
  onSelect,
  onAdd,
  onNotMe,
}: {
  machine: PendingMachine;
  selected: boolean;
  busy: boolean;
  onSelect: () => void;
  onAdd: () => void;
  onNotMe: () => void;
}) {
  const os = pendingOs(p);
  return (
    <div
      data-pending-machine={p.id}
      data-pending-kind={p.kind}
      aria-selected={selected}
      className={`rounded-lg px-2 py-[7px] ${selected ? "bg-brand/15" : "hover:bg-accent/60"}`}
    >
      <button type="button" onClick={onSelect} className="flex w-full cursor-default items-center gap-2.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/60 rounded-lg">
        <span aria-hidden className="size-2 shrink-0 rounded-full bg-warning" />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[13px] font-medium">{p.name}</span>
          <span className="block truncate text-xs text-muted-foreground" data-pending-line="">
            {p.line}
          </span>
        </span>
        {os ? <OsIcon os={os} className="size-3.5 shrink-0 text-muted-foreground/70" /> : null}
      </button>
      <div className="mt-1.5 flex gap-1.5 pl-[18px]">
        <Button size="sm" className="h-6 px-2 text-xs" disabled={busy} onClick={onAdd} data-pending-add="">
          {p.addLabel}
        </Button>
        <Button size="sm" variant="ghost" className="h-6 px-2 text-xs" disabled={busy} onClick={onNotMe} data-pending-not-me="">
          {p.notMeLabel}
        </Button>
      </div>
    </div>
  );
}

/** "Add this machine?": the waiting machine's page, with its name, system and when it asked, and two buttons. */
export function PendingDetail({ machine: p, now, busy, onAdd, onNotMe }: { machine: PendingMachine; now: number; busy: boolean; onAdd: () => void; onNotMe: () => void }) {
  return (
    <section aria-label={`${p.name} is waiting`} className="min-w-0 max-w-[620px]" data-pending-detail={p.id}>
      <h2 className="text-[22px] font-semibold tracking-[-0.015em]">Add this machine?</h2>
      <p className="mt-1.5 text-[13px] text-muted-foreground">{p.text}</p>
      <dl className="mt-5 border-y">
        <Fact label="Name" value={p.name} />
        {p.os ? <Fact label="System" value={p.os} /> : null}
        {p.since ? <Fact label="Last seen" value={relativeText(p.since, now)} /> : null}
      </dl>
      <div className="mt-5 flex gap-2">
        <Button disabled={busy} onClick={onAdd} data-pending-add="">
          {p.addLabel}
        </Button>
        <Button variant="outline" disabled={busy} onClick={onNotMe} data-pending-not-me="">
          {p.notMeLabel}
        </Button>
      </div>
    </section>
  );
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div className="grid grid-cols-[140px_1fr] border-t py-2.5 text-[13px] first:border-t-0">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="select-text">{value}</dd>
    </div>
  );
}

/** This machine's own trust, in plain words, with the one action that fixes it. */
export function TrustSection({ trust, onTrust }: { trust: ThisMachineTrust; onTrust: () => void }) {
  const status = trust.at === null ? trust.status : `${trust.status} ${dateText(trust.at)}`;
  return (
    <>
      <h3 className="mt-5 mb-2 text-xs font-semibold text-muted-foreground">Access to your machines</h3>
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

/** Who reached this machine lately: a few lines, newest first. */
export function RecentAccess({ lines, title, empty, now, error }: { lines: AccessLine[]; title: string; empty: string; now: number; error?: string | null }) {
  return (
    <>
      <h3 className="mt-7 mb-2 text-xs font-semibold text-muted-foreground">{title}</h3>
      <div className="border-y" data-machine-recent="">
        {lines.length === 0 ? <p className="py-3 text-[13px] text-muted-foreground">{empty}</p> : null}
        {lines.map((a, i) => (
          <div key={`${a.ts}-${i}`} data-recent-row="" className={`flex min-h-10 items-center justify-between gap-4 py-2 ${i > 0 ? "border-t" : ""}`}>
            <span className="flex min-w-0 items-center gap-2 text-[13px]">
              {a.notable ? <UsersIcon className="size-3.5 shrink-0 text-warning" aria-label="Another account or an untrusted machine" /> : null}
              <span className="select-text" data-recent-text="">
                {a.text}
              </span>
            </span>
            <span className="shrink-0 text-xs text-muted-foreground">{relativeText(a.ts, now)}</span>
          </div>
        ))}
        {error ? (
          <p data-devices-read-error="" className="border-t py-2.5 text-xs break-words text-destructive">
            {error}
          </p>
        ) : null}
      </div>
    </>
  );
}

/** "Trust this machine": sign in again, or add it from a machine you already trust with a one-time code. */
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
                      {o.method === "sign-in" ? <UserRoundIcon className="size-5 text-muted-foreground" /> : <HashIcon className="size-5 text-muted-foreground" />}
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
