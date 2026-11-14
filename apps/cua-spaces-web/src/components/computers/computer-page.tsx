// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link, useNavigate } from "@tanstack/react-router";
import {
  ChevronRightIcon,
  CircleAlertIcon,
  InfoIcon,
  LockIcon,
  MousePointer2Icon,
  PencilIcon,
  PlusIcon,
  ServerIcon,
  TriangleAlertIcon,
  UsersIcon,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import {
  useDevices,
  spaceDiskBytes,
  useDriver,
  useMachines,
  useNewSpaceWizard,
  useResources,
  useSession,
  useThisMachine,
  type Machine,
  type MachinesViewIssue,
  type MachinesViewMachine,
  type MachinesViewRemove,
  type MachinesViewToggle,
} from "@/bridge";
import { AddComputerSheet } from "@/components/add-computer-sheet";
import { BackLink } from "@/components/back-link";
import { computerWarnings } from "@/components/resources/computer-resources";
import { DriverAttentionLabel } from "@/components/driver/attention";
import { driverLine } from "@/components/driver/copy";
import { EnrollSheet, RecentAccess, TrustSection, useMachinesAccount } from "@/components/machines/account-parts";
import { EmptyState, Page } from "@/components/page";
import { RestartDialog, useUpdateIndicator } from "@/components/shell/update-indicator";
import { MachineRowMenu } from "@/components/spaces/space-menu";
import { ThisMachineExtras } from "@/components/this-machine";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { GroupedSection } from "@/components/ui/grouped";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { toast } from "@/components/ui/toast";
import { useMachinesLive } from "@/hooks/use-machines-live";
import { useNewSpace } from "@/hooks/use-new-space";
import { useT } from "@/i18n";
import { useHiddenDesktops } from "@/lib/hidden-desktops";
import { accessFor } from "@/lib/machines-account";
import { withRelayStatus } from "@/lib/machines-relay-status";
import { THIS_COMPUTER, computerFor } from "@/lib/machines";
import { size } from "@/lib/resources";
import { cn } from "@/lib/utils";

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** How often the page asks the computer for its health while it is open. */
const HEALTH_EVERY_MS = 60_000;

/**
 * One computer's page, opened from its row in the list (CUA-3551): its
 * switches, what is wrong with it, the Spaces made on it, New Space here,
 * Details, who reached it and Remove. Every word is the core's
 * `machines_view::view`. This Mac's carries Cua Driver too, where this Cua
 * Spaces has it.
 */
export function ComputerPage({ id }: { id: string }) {
  const t = useT();
  const { data: machines = [], isLoading, view: hostView, checkHealth } = useMachines();
  // Re-read every 30 s; a computer the relay calls offline shows offline (and can be removed) at once, as in the list.
  const { relayOnline } = useMachinesLive(true);
  const view = useMemo(() => (hostView ? withRelayStatus(hostView, machines, relayOnline) : hostView), [hostView, machines, relayOnline]);
  // "Use a code from another computer" (This Mac's setup refused for want of proof) opens Add a computer on "Enter a code".
  const [enterCode, setEnterCode] = useState(false);
  // Its issues come from its own daemon: asked on opening the page and every minute while it stays open.
  useEffect(() => {
    void checkHealth();
    const timer = setInterval(() => void checkHealth(), HEALTH_EVERY_MS);
    return () => clearInterval(timer);
    // Once per visit: the hook's functions are new on every render.
  }, []);
  const m = view ? computerFor(view.machines, id) : undefined;
  const row = m ? machines.find((r) => r.id === m.id) : undefined;
  return (
    <Page className="max-w-3xl">
      <BackLink />
      {m ? (
        <ComputerDetail key={m.id} machine={m} row={row} onUseCode={() => setEnterCode(true)} />
      ) : isLoading || !view ? (
        <div aria-busy="true" aria-label={t("computers.page.computer_loading")} className="space-y-3">
          <div className="h-7 w-56 animate-pulse rounded-lg bg-muted" />
          <div className="h-28 animate-pulse rounded-lg bg-muted" />
        </div>
      ) : (
        <EmptyState
          icon={<ServerIcon />}
          title={t("computers.page.missing_title")}
          action={<Button render={<Link to="/spaces" />}>{t("computers.page.missing_action")}</Button>}
        >
          {t("computers.page.missing_body")}
        </EmptyState>
      )}
      <EnrollSheet />
      <AddComputerSheet open={enterCode} onOpenChange={setEnterCode} initialView="enter" />
    </Page>
  );
}

/** This Mac's Cua Driver, as one of its settings: only where this Cua Spaces carries the driver (CUA-2706), with the sidebar line's word when it needs a person. */
function DriverRow({ computerId }: { computerId: string }) {
  const t = useT();
  const driver = useDriver();
  if (!driver.available) return null;
  return (
    <GroupedSection className="mt-[22px]" data-computer-driver="">
      <Link
        to="/spaces/computer/$computerId/driver"
        params={{ computerId }}
        className="flex min-h-11 items-center gap-3 px-4 py-2.5 outline-none hover:bg-accent/50 focus-visible:bg-accent/50"
        data-computer-driver-link=""
      >
        <MousePointer2Icon className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.75} />
        <span className="min-w-0 flex-1">
          <span className="block text-[13px] font-medium">{t("common.cua_driver")}</span>
          <span className="block text-xs text-muted-foreground">{driverLine()}</span>
        </span>
        <DriverAttentionLabel status={driver.status} className="shrink-0" />
        <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground/60" />
      </Link>
    </GroupedSection>
  );
}

/**
 * Green when usable, grey when not; amber when the computer has something to fix (`health`, warn or fail), which its
 * status line then says ("Online, 1 thing to fix"). Never red beside "Online": it still answers, it just needs a fix.
 */
function StatusDot({ online, health, className }: { online: boolean; health?: string; className?: string }) {
  const tint = health === "fail" || health === "warn" ? "bg-warning" : online ? "bg-success" : "bg-muted-foreground/45";
  return <span aria-hidden data-machine-health={health || undefined} className={cn("size-2 shrink-0 rounded-full", tint, className)} />;
}

function ComputerDetail({ machine: m, row, onUseCode }: { machine: MachinesViewMachine; row: Machine | undefined; onUseCode?: () => void }) {
  const t = useT();
  const { data: session } = useSession();
  const { account, now, devices } = useMachinesAccount();
  const host = useThisMachine(session?.identity ?? null);
  const { remove } = useMachines();
  const navigate = useNavigate();
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [removing, setRemoving] = useState<MachinesViewRemove | null>(null);
  const [busy, setBusy] = useState(false);
  const desktops = useHiddenDesktops();
  // Its desktop's tile on Home, which Hide takes off (per account, on this device): the switch below brings it back.
  const desktopId = m.desktop?.id ?? (m.current ? "this-mac" : null);
  const onHome = desktopId ? !desktops.hidden.has(desktopId) : true;
  const fail = (what: string) => (e: unknown) => toast(what, { description: errorText(e), type: "error" });

  const flip = (tg: MachinesViewToggle) => {
    if (tg.setup) return host.openForm(tg.setup);
    setBusy(true);
    void (async () => {
      try {
        for (const a of tg.actions) await host.run(a);
        // Turning this Mac's desktop on shows its row in the Spaces list again, if it was hidden.
        if (m.current && tg.id === "desktop" && !tg.on) desktops.show("this-mac");
      } catch (e) {
        fail(t("computers.page.toggle_failed", { name: tg.label }))(e);
      } finally {
        setBusy(false);
      }
    })();
  };
  const runRemove = (r: MachinesViewRemove) => {
    void navigate({ to: "/spaces" });
    const done = r.host ? host.run("remove") : remove(r.machine ?? null, r.device ?? null);
    void done.then(() => toast(t("computers.page.removed", { name: m.name }), { type: "success" }), fail(t("computers.page.remove_failed", { name: m.name })));
  };
  // A switch is read-only on another computer: dimmed when it can't be
  // reached at all (offline), plain with a note when it is.
  const dim = (tg: MachinesViewToggle) => !tg.editable && !m.togglesNote;
  // Another computer's Space limit, read-only: its host reports it with its other limits.
  const spaceLimit = !m.current ? row?.limits?.find((l) => l.resource === "spaces" && l.limit > 0)?.limit : undefined;
  const helpOf = (tg: MachinesViewToggle) => (tg.id === "spaces" && tg.on && spaceLimit ? t("computers.page.spaces_limit", { limit: spaceLimit }) : tg.help);

  // What each Space takes on this computer's disk, where the host says (the low-disk warning leads here).
  const resources = useResources();
  const diskWarning = row ? computerWarnings(resources, [row])[0] : undefined;
  const sizeOf = (id: string) => {
    const bytes = m.current ? spaceDiskBytes(resources, id) : null;
    return bytes ? size(bytes) : null;
  };
  // Its shared desktop first, then its Spaces, all drawn as Space rows.
  const rows: { id: string; name: string; os: string; status: string; running: boolean; label?: string }[] = [...(m.desktop ? [m.desktop] : []), ...m.spaces];

  // This computer's setup form takes the page while it is open.
  if (m.current && host.formView) {
    return (
      <section aria-label={t("computers.page.details", { name: m.name })} className="min-w-0">
        <ThisMachineExtras panel={row?.panel ?? null} progress={row?.hostProgress} onUseCode={onUseCode} />
      </section>
    );
  }

  return (
    <section aria-label={t("computers.page.details", { name: m.name })} className="min-w-0 max-w-[620px]" data-machine-detail={m.id}>
      <div className="flex items-center gap-2">
        <h2 className="truncate text-[22px] font-semibold tracking-[-0.015em]">{m.name}</h2>
        {m.rename ? (
          <button
            type="button"
            aria-label={m.rename.title}
            title={m.rename.title}
            onClick={() => setRenaming(true)}
            className="grid size-[26px] shrink-0 cursor-default place-items-center rounded-lg text-muted-foreground hover:bg-accent hover:text-foreground"
            data-machine-rename
          >
            <PencilIcon className="size-3.5" />
          </button>
        ) : null}
      </div>
      <p className="mt-1 flex items-center gap-1.5 text-[13px] text-muted-foreground" data-machine-status>
        <StatusDot online={m.online} health={m.health} className="size-[7px]" />
        {m.status}
        {m.sharedBy ? <span className="ml-2">{m.sharedBy}</span> : null}
      </p>

      {/* The full low-disk message lives here; the Computers list only marks the row. */}
      {diskWarning ? (
        <p className="mt-2 flex items-center gap-1.5 text-[13px] text-muted-foreground" data-machine-disk-warning={diskWarning.level}>
          <TriangleAlertIcon aria-hidden className="size-3.5 shrink-0 text-warning" />
          {diskWarning.text}
        </p>
      ) : null}

      <MachineIssues machine={m} hidePermissions={Boolean(m.current && row?.panel?.permissionsTitle)} onFail={fail} />

      <GroupedSection className="mt-[22px]" data-machine-toggles>
        {m.notice ? (
          <div className="flex items-center gap-2 px-4 py-2.5 text-xs text-muted-foreground" data-machine-notice>
            <InfoIcon className="size-3.5 shrink-0" />
            <span className="flex-1">{m.notice}</span>
            {m.noticeAction && m.current ? (
              <Button size="sm" variant="outline" disabled={host.busy} onClick={() => void host.run(m.noticeAction!.id).catch(fail(m.noticeAction!.label))}>
                {m.noticeAction.label}
              </Button>
            ) : null}
          </div>
        ) : null}
        {m.toggles.map((tg) => (
          <div key={tg.id} className="flex items-center justify-between gap-5 px-4 py-3.5" data-machine-toggle={tg.id}>
            <div className={cn(dim(tg) && "opacity-50")}>
              <div className="text-[13px] font-medium" id={`toggle-${tg.id}`}>
                {tg.label}
              </div>
              {helpOf(tg) ? (
                <div className="mt-px text-xs text-muted-foreground" data-machine-toggle-help={tg.id}>
                  {helpOf(tg)}
                </div>
              ) : null}
            </div>
            <Switch
              checked={tg.on}
              disabled={!tg.editable || !tg.enabled || busy || host.busy}
              aria-labelledby={`toggle-${tg.id}`}
              aria-readonly={!tg.editable || undefined}
              className={cn(!tg.editable && !dim(tg) && "data-disabled:opacity-60")}
              onCheckedChange={() => flip(tg)}
            />
          </div>
        ))}
        {desktopId && (m.desktop || !onHome) ? (
          <div className="flex items-center justify-between gap-5 px-4 py-3.5" data-machine-on-home={onHome ? "on" : "off"}>
            <div>
              <div className="text-[13px] font-medium" id="toggle-on-home">
                {t("computers.page.show_on_home")}
              </div>
              <div className="mt-px text-xs text-muted-foreground">{onHome ? t("computers.page.on_home") : t("computers.page.off_home")}</div>
            </div>
            <Switch checked={onHome} aria-labelledby="toggle-on-home" onCheckedChange={(on) => (on ? desktops.show(desktopId) : desktops.hide(desktopId))} />
          </div>
        ) : null}
        {m.togglesNote ? (
          <div className="flex items-center gap-2 px-4 py-2.5 text-xs text-muted-foreground" data-machine-toggles-note>
            {m.team ? <UsersIcon className="size-3.5" /> : <LockIcon className="size-3.5" />}
            {m.togglesNote}
          </div>
        ) : null}
      </GroupedSection>

      {m.current ? <ThisMachineExtras panel={row?.panel ?? null} progress={row?.hostProgress} part="alerts" onUseCode={onUseCode} /> : null}

      {m.current ? <DriverRow computerId={THIS_COMPUTER} /> : null}

      {m.spacesTitle ? (
        <GroupedSection title={m.spacesTitle} className="mt-7" data-machine-spaces>
          {rows.map((s) => (
            <div key={s.id} className="group/row relative">
              <Link
                to="/spaces/$spaceId"
                params={{ spaceId: s.id }}
                title={t("computers.page.open_named", { name: s.name })}
                data-space-row={s.id}
                className="flex items-center gap-2.5 px-4 py-2.5 text-[13px] outline-none hover:bg-accent/40 focus-visible:bg-accent/50"
              >
                <span className="truncate font-mono text-xs">{s.name}</span>
                {s.label ? <span className="shrink-0 text-xs text-muted-foreground">{s.label}</span> : null}
                {s.os ? <span className="truncate text-muted-foreground">{s.os}</span> : null}
                {sizeOf(s.id) ? (
                  <span className="shrink-0 text-xs text-muted-foreground tabular-nums" data-space-size="">
                    {sizeOf(s.id)}
                  </span>
                ) : null}
                <span className="ml-auto flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
                  <StatusDot online={s.running} className="size-[7px]" />
                  {s.status}
                </span>
                <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground/60 group-hover/row:invisible group-focus-within/row:invisible" />
              </Link>
              {/* Not This Mac's own desktop: it is never a notch tile. */}
              {m.current && s.id === m.desktop?.id ? null : <MachineRowMenu id={s.id} name={s.name} />}
            </div>
          ))}
          {m.spacesEmpty ? <p className="px-4 py-3.5 text-xs text-muted-foreground">{m.spacesEmpty}</p> : null}
          <div className="px-4 py-2.5">
            <NewSpaceHere machine={m} />
          </div>
        </GroupedSection>
      ) : null}

      <div className="mt-6">
        <button
          type="button"
          aria-expanded={detailsOpen}
          onClick={() => setDetailsOpen((o) => !o)}
          className="flex cursor-default items-center gap-1.5 py-1 text-xs font-semibold text-muted-foreground hover:text-foreground"
          data-machine-details-toggle
        >
          <ChevronRightIcon className={cn("size-3 transition-transform", detailsOpen && "rotate-90")} />
          {m.detailsTitle}
        </button>
        {detailsOpen ? (
          <div className="mt-2" data-machine-details>
            <GroupedSection>
              {m.details.map((f) => (
                <div key={f.label} className="grid grid-cols-[140px_1fr] px-4 py-2.5 text-[13px]">
                  <span className="text-muted-foreground">{f.label}</span>
                  <span className="select-text">{f.value}</span>
                </div>
              ))}
            </GroupedSection>
            {m.current && account?.thisMachine ? <TrustSection trust={account.thisMachine} onTrust={devices.startEnroll} /> : null}
            {m.current ? <ThisMachineExtras panel={row?.panel ?? null} part="logs" /> : null}
          </div>
        ) : null}
      </div>

      <div className="mt-7 flex flex-wrap items-center gap-x-5" data-machine-actions="">
        {account ? (
          <RecentAccess
            lines={accessFor(account.access, m.current ? (row?.relayId ?? m.id) : m.id, m.current ? null : row?.device ? m.id : null)}
            title={account.recentTitle}
            empty={account.recentEmpty}
            now={now}
            error={m.current && account.thisMachine?.kind === "enrolled" ? account.readError : null}
          />
        ) : null}
        {m.remove ? (
          <span title={m.remove.help ?? undefined} className="inline-flex">
            <button
              type="button"
              disabled={!m.remove.enabled}
              onClick={() => setRemoving(m.remove!)}
              className="cursor-default py-1 text-[13px] text-destructive hover:underline hover:underline-offset-2 disabled:opacity-50 disabled:hover:no-underline"
              data-machine-remove={m.id}
            >
              {m.remove.label}
            </button>
          </span>
        ) : null}
      </div>

      {removing ? (
        <ConfirmDialog
          open
          onOpenChange={(o) => !o && setRemoving(null)}
          title={removing.confirm.title}
          description={removing.confirm.message}
          confirmLabel={removing.confirm.confirmLabel}
          cancelLabel={removing.confirm.cancelLabel}
          destructive
          onConfirm={() => runRemove(removing)}
        />
      ) : null}
      {m.rename ? <RenameDialog open={renaming} machine={m} onClose={() => setRenaming(false)} /> : null}
    </section>
  );
}

/**
 * What is wrong with the computer, from its own daemon: one sentence each,
 * with its fix where there is one here. Nothing at all when it is healthy.
 * `hidePermissions`: This Mac lists the permissions it needs already.
 */
function MachineIssues({
  machine: m,
  hidePermissions,
  onFail,
}: {
  machine: MachinesViewMachine;
  hidePermissions: boolean;
  onFail: (what: string) => (e: unknown) => void;
}) {
  const t = useT();
  const { data: session } = useSession();
  const host = useThisMachine(session?.identity ?? null);
  const issues = (m.issues ?? []).filter((i) => !(hidePermissions && i.id.startsWith("machine.permissions.")));
  const update = useUpdateIndicator();
  if (!issues.length) return null;
  const act = (i: MachinesViewIssue) => {
    const a = i.action!;
    // Restart into the downloaded Cua Spaces update: the sidebar's own flow, which asks first while an agent is running.
    if (a.appUpdate) return update.restart();
    const done = a.url ? host.openSettings(a.url) : a.hostAction ? host.run(a.hostAction) : Promise.resolve();
    void Promise.resolve(done).catch(onFail(t("computers.page.action_failed", { action: a.label.charAt(0).toLowerCase() + a.label.slice(1) })));
  };
  return (
    <>
      <GroupedSection title={m.issuesTitle || t("computers.page.issues")} className="mt-[22px]" data-machine-issues>
        {issues.map((i) => (
          <div key={i.id} className="flex items-center gap-2.5 px-4 py-2.5" data-machine-issue={i.id} data-level={i.level}>
            {i.level === "fail" ? (
              <CircleAlertIcon aria-label={t("computers.page.issue_problem")} className="size-3.5 shrink-0 text-destructive" />
            ) : i.level === "info" ? (
              <InfoIcon aria-label={t("computers.page.issue_note")} className="size-3.5 shrink-0 text-muted-foreground" />
            ) : (
              <TriangleAlertIcon aria-label={t("computers.page.issue_warning")} className="size-3.5 shrink-0 text-warning" />
            )}
            <span className="min-w-0 flex-1 text-[13px]">{i.text}</span>
            {i.action ? (
              <Button
                size="sm"
                variant="outline"
                className="shrink-0"
                disabled={host.busy || (i.action.appUpdate && update.installing)}
                onClick={() => act(i)}
                data-machine-issue-action={i.id}
              >
                {i.action.label}
              </Button>
            ) : null}
          </div>
        ))}
      </GroupedSection>
      <RestartDialog ui={update} />
    </>
  );
}

/** New Space here: the wizard with this computer chosen. */
function NewSpaceHere({ machine: m }: { machine: MachinesViewMachine }) {
  const wizard = useNewSpaceWizard();
  const newSpace = useNewSpace();
  const ns = m.newSpace;
  // Without the wizard, only this computer can be chosen (the host's own create).
  const enabled = ns.enabled && (wizard.offered || ns.on === "local");
  return (
    <span title={ns.help ?? undefined} className="inline-flex">
      <Button variant="outline" size="sm" disabled={!enabled} onClick={() => (wizard.offered ? wizard.show(ns.on) : newSpace())} data-machine-new-space={ns.on}>
        <PlusIcon /> {ns.label}
      </Button>
    </span>
  );
}

function RenameDialog({ open, machine: m, onClose }: { open: boolean; machine: MachinesViewMachine; onClose: () => void }) {
  const t = useT();
  const { rename, refresh } = useMachines();
  const devices = useDevices();
  const [name, setName] = useState(m.name);
  useEffect(() => {
    if (open) setName(m.name);
  }, [open, m.name]);
  const r = m.rename!;
  const save = () => {
    const clean = name.trim();
    if (!clean || clean === m.name) return onClose();
    onClose();
    const done = r.target === "device" ? devices.rename(r.id, clean).then(refresh) : rename(r.id, clean);
    void done.then(
      () => toast(t("computers.page.renamed", { name: clean }), { type: "success" }),
      (e: unknown) => toast(t("computers.page.computer_rename_failed"), { description: errorText(e), type: "error" }),
    );
  };
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogPopup className="w-[min(380px,calc(100vw-2rem))]">
        <form
          onSubmit={(e) => {
            e.preventDefault();
            save();
          }}
        >
          <div className="px-5 pt-5 pb-4">
            <DialogTitle className="text-[15px] font-semibold">{r.title}</DialogTitle>
            <Input
              className="mt-3"
              aria-label={r.title}
              value={name}
              autoFocus
              maxLength={64}
              onChange={(e) => setName(e.currentTarget.value)}
              data-machine-rename-input
            />
          </div>
          {/* design-ok: a dialog's footer bar, as ConfirmDialog's */}
          <div className="flex justify-end gap-2 border-t bg-muted/60 px-5 py-3">
            <Button type="button" variant="outline" onClick={onClose}>
              {t("computers.page.cancel")}
            </Button>
            <Button type="submit" disabled={!name.trim()}>
              {r.confirmLabel}
            </Button>
          </div>
        </form>
      </DialogPopup>
    </Dialog>
  );
}
