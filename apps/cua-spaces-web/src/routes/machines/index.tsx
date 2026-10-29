// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link, createFileRoute } from "@tanstack/react-router";
import { ChevronRightIcon, CircleAlertIcon, InfoIcon, LockIcon, PencilIcon, PlusIcon, SearchIcon, ServerIcon, TriangleAlertIcon, UsersIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import {
  useConnectCloud,
  useDevices,
  useMachines,
  useNewSpaceWizard,
  useSession,
  useThisMachine,
  type CloudsView,
  type Machine,
  type MachinesView,
  type MachinesViewIssue,
  type MachinesViewMachine,
  type MachinesViewRemove,
  type MachinesViewToggle,
  type PendingMachine,
} from "@/bridge";
import { AddComputerSheet } from "@/components/add-computer-sheet";
import { EnrollSheet, PendingDetail, PendingRow, RecentAccess, TrustSection, useMachinesAccount, usePendingActions } from "@/components/machines/account-parts";
import { accessFor } from "@/lib/machines-account";
import { CloudDetail, CloudDot } from "@/components/clouds/cloud-detail";
import { CloudLogo } from "@/components/cloud-logo";
import { UpsellPrompt } from "@/components/upsell-prompt";
import { OsIcon } from "@/components/os-icon";
import { EmptyState, Page, PageHeader } from "@/components/page";
import { RestartDialog, useUpdateIndicator } from "@/components/shell/update-indicator";
import { MachineRowMenu } from "@/components/spaces/space-menu";
import { ThisMachineExtras } from "@/components/this-machine";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { toast } from "@/components/ui/toast";
import { useNewSpace } from "@/hooks/use-new-space";
import { showDesktop } from "@/lib/hidden-desktops";
import { machineOs, selectedMachine } from "@/lib/machines";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/machines/")({
  component: MachinesPage,
  // `?add=1` opens "Add a computer".
  // `?join=<code>` opens it on "Enter a code" with the code filled in (a `cua://add/<code>`
  // link opened on this computer); `relay` names a relay other than the production one.
  validateSearch: (s: Record<string, unknown>): { add?: true; join?: string; relay?: string } => ({
    ...(s.add === 1 || s.add === "1" || s.add === true ? { add: true as const } : {}),
    ...(typeof s.join === "string" && s.join ? { join: s.join.slice(0, 200) } : {}),
    ...(typeof s.relay === "string" && s.relay ? { relay: s.relay.slice(0, 300) } : {}),
  }),
});

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The view without the sign-ins that are waiting to be added (they are the top group). */
function withoutPending(view: MachinesView, pending: readonly PendingMachine[]): MachinesView {
  const waiting = new Set(pending.flatMap((p) => (p.kind === "sign-in" && p.device ? [p.device] : [])));
  if (waiting.size === 0) return view;
  return {
    ...view,
    machines: view.machines.filter((m) => !waiting.has(m.id)),
    groups: view.groups.map((g) => ({ ...g, ids: g.ids.filter((id) => !waiting.has(id)) })).filter((g) => g.ids.length > 0),
  };
}

/** "3 machines, 2 online", and what waits. */
function summaryText(view: MachinesView | null, waiting: number): string {
  const total = view?.machines.length ?? 0;
  const online = view?.machines.filter((m) => m.online).length ?? 0;
  const base = `${total} machine${total === 1 ? "" : "s"}, ${online} online`;
  return waiting > 0 ? `${base}, ${waiting} waiting` : base;
}

/** How often the page asks the machines for their health while it is open. */
const HEALTH_EVERY_MS = 60_000;

/**
 * Machines: the hardware. The list in groups (your machines, the ones
 * offline for 30+ days, team machines) with one line each, and the
 * selected machine's page: its two switches, the Spaces on it as links,
 * New Space here, Details and Remove from account. Everything shown is the
 * core's `machines_view::view`, as the SwiftUI page draws it. A Space's own
 * actions (open, start, stop, teleport, drop files) stay on the Space.
 */
function MachinesPage() {
  const { data: machines = [], isLoading, error, view: fullView, checkHealth, remove } = useMachines();
  const { account, pending, now } = useMachinesAccount();
  const actions = usePendingActions();
  // A machine waiting to be added is in the top group, not in the list below.
  const view = useMemo(() => (fullView ? withoutPending(fullView, pending) : fullView), [fullView, pending]);
  // Each machine's issues come from its own daemon: asked on opening the
  // page and every minute while it stays open.
  useEffect(() => {
    void checkHealth();
    const timer = setInterval(() => void checkHealth(), HEALTH_EVERY_MS);
    return () => clearInterval(timer);
    // Once per visit: the hook's functions are new on every render.
  }, []);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const { add, join, relay } = Route.useSearch();
  const navigate = Route.useNavigate();
  const [adding, setAddingState] = useState(add === true || join !== undefined);
  const setAdding = (open: boolean) => {
    setAddingState(open);
    if (!open && (add || join)) void navigate({ search: {}, replace: true });
  };
  useEffect(() => {
    if (add || join) setAddingState(true);
  }, [add, join]);
  const shown = view?.machines ?? [];
  // A cloud is selected as `cloud:<id>`; a machine by its id.
  const cloudPage = selectedId?.startsWith("cloud:") ? view?.clouds?.pages.find((c) => c.id === selectedId.slice(6)) : undefined;
  const pendingPage = selectedId?.startsWith("pending:") ? pending.find((p) => `pending:${p.id}` === selectedId) : undefined;
  const selected = cloudPage || pendingPage ? undefined : selectedMachine(shown, selectedId?.startsWith("cloud:") || selectedId?.startsWith("pending:") ? null : selectedId);
  const row = (id: string) => machines.find((m) => m.id === id);

  return (
    <Page contained className="max-w-6xl">
      <PageHeader
        title="Machines"
        description={isLoading ? "Loading machines…" : view?.machines.length || pending.length ? summaryText(view, pending.length) : "Machines that can run your Spaces."}
        actions={
          <Button onClick={() => setAdding(true)} data-machines-add>
            <PlusIcon /> Add machine
          </Button>
        }
      />
      {error ? (
        <EmptyState icon={<ServerIcon />} title="Couldn't load machines">
          {error.message}
        </EmptyState>
      ) : isLoading || (!view && machines.length > 0) ? (
        <LoadingState />
      ) : !view || (view.machines.length === 0 && pending.length === 0) ? (
        <EmptyState
          icon={<ServerIcon />}
          title="Add your first machine"
          action={
            <div className="flex gap-2">
              <Button onClick={() => setAdding(true)} data-machines-add-this>
                <PlusIcon /> This machine
              </Button>
              <Button variant="outline" onClick={() => setAdding(true)} data-machines-add-other>
                Another machine
              </Button>
            </div>
          }
        >
          Set up a Mac, Linux or Windows machine and it shows here, ready to run Spaces.
        </EmptyState>
      ) : (
        <div className="grid min-h-0 flex-1 grid-cols-[minmax(0,320px)_minmax(0,1fr)] grid-rows-[minmax(0,1fr)] gap-8" data-machines-split="">
          <div className="min-h-0 overflow-y-auto overscroll-contain pr-1 pb-10" data-machines-list-scroll="">
            <MachineList
              view={view}
              rows={machines}
              query={query}
              onQuery={setQuery}
              selectedId={cloudPage ? `cloud:${cloudPage.id}` : pendingPage ? `pending:${pendingPage.id}` : (selected?.id ?? null)}
              onSelect={setSelectedId}
              pending={pending}
              pendingTitle={account?.pendingTitle ?? "Waiting for you"}
              pendingBusy={actions.busy}
              onAdd={actions.add}
              onNotMe={actions.notMe}
            />
            <UpsellPrompt surface="machine" active={machines.length >= 2 && machines.some((m) => m.via === "relay")} className="mt-3" />
          </div>
          <div className="min-h-0 overflow-y-auto overscroll-contain pb-10" data-machines-detail-scroll="">
            {cloudPage ? (
              <CloudDetail key={cloudPage.id} page={cloudPage} onGone={() => setSelectedId(null)} />
            ) : pendingPage ? (
              <PendingDetail key={pendingPage.id} machine={pendingPage} now={now} busy={actions.busy === pendingPage.id} onAdd={() => actions.add(pendingPage)} onNotMe={() => actions.notMe(pendingPage)} />
            ) : selected ? (
              <MachineDetail key={selected.id} machine={selected} row={row(selected.id)} onRemoved={() => setSelectedId(null)} />
            ) : null}
          </div>
        </div>
      )}
      <EnrollSheet />
      {actions.notMeDialog((machine) => remove(machine, null))}
      <AddComputerSheet open={adding} onOpenChange={setAdding} initialView={join !== undefined ? "enter" : "pick"} initialCode={join ?? ""} initialRelay={relay ?? null} />
    </Page>
  );
}

/* ---- The list ---------------------------------------------------------------- */

function MachineList({
  view,
  rows,
  query,
  onQuery,
  selectedId,
  onSelect,
  pending,
  pendingTitle,
  pendingBusy,
  onAdd,
  onNotMe,
}: {
  view: MachinesView;
  rows: Machine[];
  query: string;
  onQuery: (q: string) => void;
  selectedId: string | null;
  onSelect: (id: string) => void;
  pending: PendingMachine[];
  pendingTitle: string;
  pendingBusy: string | null;
  onAdd: (p: PendingMachine) => void;
  onNotMe: (p: PendingMachine) => void;
}) {
  const { remove } = useMachines();
  const connect = useConnectCloud();
  const [openOld, setOpenOld] = useState(false);
  const [removingOld, setRemovingOld] = useState(false);
  const byId = useMemo(() => new Map(view.machines.map((m) => [m.id, m] as const)), [view]);
  const q = query.trim().toLowerCase();
  const matches = (m: MachinesViewMachine) => !q || `${m.name} ${m.line}`.toLowerCase().includes(q);
  const removeOld = view.groups.find((g) => g.removeAll)?.removeAll ?? null;
  const clouds = view.clouds;
  // The selected row stays in view in the list's own scroll (arrow keys, a click on a cloud, a new selection).
  const listRef = useRef<HTMLElement>(null);
  useEffect(() => {
    if (!selectedId) return;
    const sel = selectedId.startsWith("cloud:") ? `[data-cloud-id="${CSS.escape(selectedId.slice(6))}"]` : `[data-machine-id="${CSS.escape(selectedId)}"]`;
    listRef.current?.querySelector<HTMLElement>(sel)?.scrollIntoView?.({ block: "nearest" });
  }, [selectedId]);
  const cloudMatches = (c: CloudsView["group"]["rows"][number]) => !q || `${c.name} ${c.line}`.toLowerCase().includes(q);
  const cloudIds = (clouds?.group.rows ?? []).filter(cloudMatches).map((c) => `cloud:${c.id}`);
  const ids = view.machines.map((m) => m.id);
  const step = (from: string, by: number) => {
    const visible = [
      ...ids.filter((id) => matches(byId.get(id)!) && (openOld || !view.groups.find((g) => g.collapsed)?.ids.includes(id))),
      ...cloudIds,
    ];
    const i = visible.indexOf(from);
    const next = visible[Math.max(0, Math.min(visible.length - 1, i + by))];
    if (next) {
      onSelect(next);
      document.querySelector<HTMLElement>(`[data-machine-id="${CSS.escape(next)}"], [data-cloud-id="${CSS.escape(next.replace("cloud:", ""))}"]`)?.focus();
    }
  };

  // The clouds come first (the core's `order`): a long machine list never pushes them out of view.
  const cloudsSection = clouds && (cloudIds.length > 0 || !q) ? (
          <div key="clouds" data-cloud-group="clouds">
            <div className="px-2 pt-3.5 pb-1 text-[11px] font-semibold tracking-[0.02em] text-muted-foreground">{clouds.group.title}</div>
            {clouds.group.rows.filter(cloudMatches).map((c) => (
              <CloudRow key={c.id} cloud={c} selected={selectedId === `cloud:${c.id}`} onSelect={() => onSelect(`cloud:${c.id}`)} onStep={(by) => step(`cloud:${c.id}`, by)} />
            ))}
            {!q ? (
              <button
                type="button"
                onClick={() => connect.show()}
                data-connect-a-cloud=""
                className="flex w-full cursor-default items-center gap-2.5 rounded-lg px-2 py-[7px] text-left text-[13px] text-muted-foreground outline-none transition-colors hover:bg-accent/60 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
              >
                <PlusIcon className="size-3.5 shrink-0" />
                {clouds.group.connectLabel}
              </button>
            ) : null}
          </div>
        ) : null;

  return (
    <section ref={listRef} aria-label="Machine list" className="min-w-0">
      <label className="mb-1 flex h-[30px] items-center gap-2 rounded-lg border bg-card px-2.5 text-muted-foreground focus-within:border-brand focus-within:ring-[3px] focus-within:ring-brand/15">
        <SearchIcon className="size-3.5 shrink-0" />
        <input
          type="search"
          value={query}
          onChange={(e) => onQuery(e.currentTarget.value)}
          placeholder="Search machines"
          aria-label="Search machines"
          className="min-w-0 flex-1 bg-transparent text-[13px] text-foreground outline-none placeholder:text-muted-foreground/70"
        />
      </label>
      {pending.length > 0 ? (
        <div data-machine-group="pending" data-pending-count={pending.length}>
          <div className="px-2 pt-3.5 pb-1 text-[11px] font-semibold tracking-[0.02em] text-muted-foreground">{pendingTitle}</div>
          {pending
            .filter((p) => !q || p.name.toLowerCase().includes(q))
            .map((p) => (
              <PendingRow key={p.id} machine={p} selected={selectedId === `pending:${p.id}`} busy={pendingBusy === p.id} onSelect={() => onSelect(`pending:${p.id}`)} onAdd={() => onAdd(p)} onNotMe={() => onNotMe(p)} />
            ))}
        </div>
      ) : null}
      <div role="listbox" aria-label="Machines">
        {(view.order ?? ["clouds", ...view.groups.map((g) => g.id)]).map((section) => {
          if (section === "clouds") return cloudsSection;
          const g = view.groups.find((x) => x.id === section);
          if (!g) return null;
          const members = g.ids.map((id) => byId.get(id)).filter((m): m is MachinesViewMachine => Boolean(m));
          const visible = members.filter(matches);
          if (!visible.length && q) return null;
          const open = !g.collapsed || openOld || Boolean(q);
          return (
            <div key={g.id} data-machine-group={g.id}>
              {g.collapsed ? (
                <div className="mt-1 flex items-center justify-between">
                  <button
                    type="button"
                    aria-expanded={open}
                    onClick={() => setOpenOld((o) => !o)}
                    className="flex flex-1 cursor-default items-center gap-1.5 rounded-md px-2 py-1.5 text-left text-xs text-muted-foreground hover:bg-accent/60"
                  >
                    <ChevronRightIcon className={cn("size-3 transition-transform", open && "rotate-90")} />
                    {g.title}
                  </button>
                  {g.removeAll ? (
                    <button
                      type="button"
                      onClick={() => setRemovingOld(true)}
                      className="shrink-0 cursor-default px-1 text-xs text-muted-foreground underline decoration-border underline-offset-2 hover:text-foreground"
                      data-machines-remove-old
                    >
                      {g.removeAll.label}
                    </button>
                  ) : null}
                </div>
              ) : (
                <div className="px-2 pt-3.5 pb-1 text-[11px] font-semibold tracking-[0.02em] text-muted-foreground">{g.title}</div>
              )}
              {visible.map((m) => (
                <MachineRow
                  key={m.id}
                  machine={m}
                  row={rows.find((r) => r.id === m.id)}
                  hidden={!open}
                  selected={m.id === selectedId}
                  onSelect={() => onSelect(m.id)}
                  onStep={(by) => step(m.id, by)}
                />
              ))}
            </div>
          );
        })}
      </div>
      {removeOld ? (
        <ConfirmDialog
          open={removingOld}
          onOpenChange={setRemovingOld}
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
                  toast("Couldn't remove a machine", { description: errorText(e), type: "error" });
                  return;
                }
              }
              toast(removeOld.removes.length === 1 ? "Removed 1 old machine" : `Removed ${removeOld.removes.length} old machines`, { type: "success" });
            })();
          }}
        />
      ) : null}
    </section>
  );
}

/** A cloud in the list: its dot, name, one short line, and its logo at the size of the OS icons. */
function CloudRow({
  cloud: c,
  selected,
  onSelect,
  onStep,
}: {
  cloud: CloudsView["group"]["rows"][number];
  selected: boolean;
  onSelect: () => void;
  onStep: (by: number) => void;
}) {
  return (
    <button
      type="button"
      role="option"
      aria-selected={selected}
      data-cloud-id={c.id}
      data-health={c.health}
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          onStep(e.key === "ArrowDown" ? 1 : -1);
        }
      }}
      className={cn(
        "flex w-full cursor-default items-center gap-2.5 rounded-lg px-2 py-[7px] text-left outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/60",
        selected ? "bg-brand/15" : "hover:bg-accent/60",
      )}
    >
      <CloudDot health={c.health} />
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[13px] font-medium">{c.name}</span>
        <span className="block truncate text-xs text-muted-foreground" data-cloud-line="">
          {c.line}
        </span>
      </span>
      <CloudLogo cloud={c.logo} size={14} />
    </button>
  );
}

/** Green when usable, grey when not; amber or red when the machine has
 * issues (`health`). */
function StatusDot({ online, health, className }: { online: boolean; health?: string; className?: string }) {
  const tint = health === "fail" ? "bg-destructive" : health === "warn" ? "bg-warning" : online ? "bg-success" : "bg-muted-foreground/45";
  return <span aria-hidden data-machine-health={health || undefined} className={cn("size-2 shrink-0 rounded-full", tint, className)} />;
}

function MachineRow({
  machine: m,
  row,
  hidden,
  selected,
  onSelect,
  onStep,
}: {
  machine: MachinesViewMachine;
  row: Machine | undefined;
  hidden: boolean;
  selected: boolean;
  onSelect: () => void;
  onStep: (by: number) => void;
}) {
  const os = machineOs(m);
  return (
    <button
      type="button"
      role="option"
      hidden={hidden}
      aria-selected={selected}
      data-machine-id={m.id}
      data-online={(row?.online ?? m.online) ? "true" : "false"}
      data-not-sharing={row?.notSharing ? "true" : undefined}
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          onStep(e.key === "ArrowDown" ? 1 : -1);
        }
      }}
      className={cn(
        "flex w-full cursor-default items-center gap-2.5 rounded-lg px-2 py-[7px] text-left outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/60",
        selected ? "bg-brand/15" : "hover:bg-accent/60",
      )}
    >
      <StatusDot online={m.online} health={m.health} />
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-1.5">
          <span className="truncate text-[13px] font-medium">{m.name}</span>
          {m.badge ? <span className="shrink-0 rounded border bg-muted px-1.5 text-[10px] font-medium text-muted-foreground">{m.badge}</span> : null}
        </span>
        <span className="block truncate text-xs text-muted-foreground" data-machine-line>
          {m.line}
        </span>
      </span>
      {os ? <OsIcon os={os} className="size-3.5 shrink-0 text-muted-foreground/70" /> : null}
    </button>
  );
}

/* ---- A machine's page --------------------------------------------------------------- */

function MachineDetail({ machine: m, row, onRemoved }: { machine: MachinesViewMachine; row: Machine | undefined; onRemoved: () => void }) {
  const { data: session } = useSession();
  const { account, now, devices } = useMachinesAccount();
  const host = useThisMachine(session?.identity ?? null);
  const { remove } = useMachines();
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [removing, setRemoving] = useState<MachinesViewRemove | null>(null);
  const [busy, setBusy] = useState(false);
  const fail = (what: string) => (e: unknown) => toast(what, { description: errorText(e), type: "error" });

  const flip = (t: MachinesViewToggle) => {
    if (t.setup) return host.openForm(t.setup);
    setBusy(true);
    void (async () => {
      try {
        for (const a of t.actions) await host.run(a);
        // Turning this Mac's desktop on shows its row in the Spaces list again, if it was hidden.
        if (m.current && t.id === "desktop" && !t.on) showDesktop("this-mac");
      } catch (e) {
        fail(`Couldn't change ${t.label}`)(e);
      } finally {
        setBusy(false);
      }
    })();
  };
  const runRemove = (r: MachinesViewRemove) => {
    onRemoved();
    const done = r.host ? host.run("remove") : remove(r.machine ?? null, r.device ?? null);
    void done.then(() => toast(`Removed ${m.name}`, { type: "success" }), fail(`Couldn't remove ${m.name}`));
  };
  // A switch is read-only on another machine: dimmed when it can't be
  // reached at all (offline), plain with a note when it is.
  const dim = (t: MachinesViewToggle) => !t.editable && !m.togglesNote;
  // Another machine's Space limit, read-only: its host reports it with its other limits.
  const spaceLimit = !m.current ? row?.limits?.find((l) => l.resource === "spaces" && l.limit > 0)?.limit : undefined;
  const helpOf = (t: MachinesViewToggle) => (t.id === "spaces" && t.on && spaceLimit ? `Spaces can be created here (up to ${spaceLimit})` : t.help);

  // Its shared desktop first, then its Spaces, all drawn as Space rows.
  const rows: { id: string; name: string; os: string; status: string; running: boolean; label?: string }[] = [...(m.desktop ? [m.desktop] : []), ...m.spaces];

  // This machine's setup form takes the page while it is open.
  if (m.current && host.formView) {
  return (
      <section aria-label={`${m.name} details`} className="min-w-0">
        <ThisMachineExtras panel={row?.panel ?? null} progress={row?.hostProgress} />
      </section>
    );
  }

  return (
    <section aria-label={`${m.name} details`} className="min-w-0 max-w-[620px]" data-machine-detail={m.id}>
      <div className="flex items-center gap-2">
        <h2 className="truncate text-[22px] font-semibold tracking-[-0.015em]">{m.name}</h2>
        {m.rename ? (
          <button
            type="button"
            aria-label={m.rename.title}
            title={m.rename.title}
            onClick={() => setRenaming(true)}
            className="grid size-[26px] shrink-0 cursor-default place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
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

      <MachineIssues machine={m} hidePermissions={Boolean(m.current && row?.panel?.permissionsTitle)} onFail={fail} />

      <div className="mt-[22px] border-y" data-machine-toggles>
        {m.notice ? (
          <div className="flex items-center gap-2 border-b py-2.5 text-xs text-muted-foreground" data-machine-notice>
            <InfoIcon className="size-3.5 shrink-0" />
            <span className="flex-1">{m.notice}</span>
            {m.noticeAction && m.current ? (
              <Button size="sm" variant="outline" disabled={host.busy} onClick={() => void host.run(m.noticeAction!.id).catch(fail(m.noticeAction!.label))}>
                {m.noticeAction.label}
              </Button>
            ) : null}
          </div>
        ) : null}
        {m.toggles.map((t, i) => (
          <div key={t.id} className={cn("flex items-center justify-between gap-5 py-3.5", i > 0 && "border-t")} data-machine-toggle={t.id}>
            <div className={cn(dim(t) && "opacity-50")}>
              <div className="text-[13px] font-medium" id={`toggle-${t.id}`}>
                {t.label}
              </div>
              <div className="mt-px text-xs text-muted-foreground" data-machine-toggle-help={t.id}>
                {helpOf(t)}
              </div>
            </div>
            <Switch
              checked={t.on}
              disabled={!t.editable || !t.enabled || busy || host.busy}
              aria-labelledby={`toggle-${t.id}`}
              aria-readonly={!t.editable || undefined}
              className={cn(!t.editable && !dim(t) && "data-disabled:opacity-60")}
              onCheckedChange={() => flip(t)}
            />
          </div>
        ))}
        {m.togglesNote ? (
          <div className="flex items-center gap-2 border-t py-2.5 text-xs text-muted-foreground" data-machine-toggles-note>
            {m.team ? <UsersIcon className="size-3.5" /> : <LockIcon className="size-3.5" />}
            {m.togglesNote}
          </div>
        ) : null}
      </div>

      {m.current ? <ThisMachineExtras panel={row?.panel ?? null} progress={row?.hostProgress} part="alerts" /> : null}

      {m.spacesTitle ? (
        <>
          <h3 className="mt-7 mb-2 text-xs font-semibold text-muted-foreground">{m.spacesTitle}</h3>
          <div className="border-y" data-machine-spaces>
            {rows.map((s, i) => (
              <div key={s.id} className={cn("group/row relative", i > 0 && "border-t")}>
                <Link
                  to="/spaces/$spaceId"
                  params={{ spaceId: s.id }}
                  title={`Open ${s.name}`}
                  data-space-row={s.id}
                  className="flex items-center gap-2.5 py-2.5 text-[13px] outline-none hover:bg-accent/40 focus-visible:bg-accent/50"
                >
                  <span className="truncate font-mono text-xs">{s.name}</span>
                  {s.label ? <span className="shrink-0 rounded bg-muted px-1.5 text-2xs text-muted-foreground">{s.label}</span> : null}
                  {s.os ? <span className="truncate text-muted-foreground">{s.os}</span> : null}
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
            {m.spacesEmpty ? <p className={cn("py-3.5 text-xs text-muted-foreground", rows.length > 0 && "border-t")}>{m.spacesEmpty}</p> : null}
            <div className="border-t py-2.5">
              <NewSpaceHere machine={m} />
            </div>
          </div>
        </>
      ) : null}

      {account ? <RecentAccess lines={accessFor(account.access, m.current ? (row?.relayId ?? m.id) : m.id, m.current ? null : row?.device ? m.id : null)} title={account.recentTitle} empty={account.recentEmpty} now={now} error={m.current && account.thisMachine?.kind === "enrolled" ? account.readError : null} /> : null}

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
            <dl className="border-y">
              {m.details.map((f, i) => (
                <div key={f.label} className={cn("grid grid-cols-[140px_1fr] py-2.5 text-[13px]", i > 0 && "border-t")}>
                  <dt className="text-muted-foreground">{f.label}</dt>
                  <dd className="select-text">{f.value}</dd>
                </div>
              ))}
            </dl>
            {m.current && account?.thisMachine ? <TrustSection trust={account.thisMachine} onTrust={devices.startEnroll} /> : null}
            {m.current ? <ThisMachineExtras panel={row?.panel ?? null} part="logs" /> : null}
          </div>
        ) : null}
      </div>

      {m.remove ? (
        <span title={m.remove.help ?? undefined} className="mt-7 inline-flex">
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

/** New Space here: the wizard with this machine chosen. */
/**
 * What is wrong with the machine, from its own daemon: one sentence each,
 * with its fix where there is one here. Nothing at all when it is healthy.
 * `hidePermissions`: This Mac lists the permissions it needs already.
 */
function MachineIssues({ machine: m, hidePermissions, onFail }: { machine: MachinesViewMachine; hidePermissions: boolean; onFail: (what: string) => (e: unknown) => void }) {
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
    void Promise.resolve(done).catch(onFail(`Couldn't ${a.label.charAt(0).toLowerCase()}${a.label.slice(1)}`));
  };
  return (
    <>
      <h3 className="mt-[22px] mb-2 text-xs font-semibold text-muted-foreground">{m.issuesTitle || "Issues"}</h3>
      <div className="border-y" data-machine-issues>
        {issues.map((i, n) => (
          <div key={i.id} className={cn("flex items-center gap-2.5 py-2.5", n > 0 && "border-t")} data-machine-issue={i.id} data-level={i.level}>
            {i.level === "fail" ? (
              <CircleAlertIcon aria-label="Problem" className="size-3.5 shrink-0 text-destructive" />
            ) : i.level === "info" ? (
              <InfoIcon aria-label="Note" className="size-3.5 shrink-0 text-muted-foreground" />
            ) : (
              <TriangleAlertIcon aria-label="Warning" className="size-3.5 shrink-0 text-warning" />
            )}
            <span className="min-w-0 flex-1 text-[13px]">{i.text}</span>
            {i.action ? (
              <Button size="sm" variant="outline" className="shrink-0" disabled={host.busy || (i.action.appUpdate && update.installing)} onClick={() => act(i)} data-machine-issue-action={i.id}>
                {i.action.label}
              </Button>
            ) : null}
          </div>
        ))}
      </div>
      <RestartDialog ui={update} />
    </>
  );
}

function NewSpaceHere({ machine: m }: { machine: MachinesViewMachine }) {
  const wizard = useNewSpaceWizard();
  const newSpace = useNewSpace();
  const ns = m.newSpace;
  // Without the wizard, only this machine can be chosen (the host's own create).
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
      () => toast(`Renamed to ${clean}`, { type: "success" }),
      (e: unknown) => toast("Couldn't rename the machine", { description: errorText(e), type: "error" }),
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
            <Input className="mt-3" aria-label={r.title} value={name} autoFocus maxLength={64} onChange={(e) => setName(e.currentTarget.value)} data-machine-rename-input />
          </div>
          <div className="flex justify-end gap-2 border-t bg-muted/60 px-5 py-3">
            <Button type="button" variant="outline" onClick={onClose}>
              Cancel
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

function LoadingState() {
  return (
    <div aria-busy="true" aria-label="Loading machines" className="grid grid-cols-[minmax(0,320px)_minmax(0,1fr)] items-start gap-8">
      <div className="space-y-1">
        <div className="mb-3 h-[30px] animate-pulse rounded-lg bg-muted" />
        {[0, 1, 2].map((i) => (
          <div key={i} className="flex items-center gap-2.5 px-2 py-2">
            <div className="size-2 animate-pulse rounded-full bg-muted" />
            <div className="flex-1 space-y-1.5">
              <div className="h-3 w-32 animate-pulse rounded bg-muted" />
              <div className="h-2.5 w-24 animate-pulse rounded bg-muted" />
            </div>
          </div>
        ))}
      </div>
      <div className="space-y-3">
        <div className="h-7 w-56 animate-pulse rounded-lg bg-muted" />
        <div className="h-28 animate-pulse rounded-xl bg-muted" />
      </div>
    </div>
  );
}

