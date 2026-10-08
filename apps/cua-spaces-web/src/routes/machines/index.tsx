// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";
import { LaptopIcon, PlusIcon, ServerIcon, Trash2Icon } from "lucide-react";
import { useState, type ReactNode } from "react";

import { useBridge, useExperiments, useMachines, useSpaces, type Machine, type Space, type SpaceDetailView } from "@/bridge";
import { spaceDetail } from "@/bridge/space-detail";
import { AddMachineDialog } from "@/components/add-machine-dialog";
import { ThisMachine } from "@/components/this-machine";
import { OsIcon } from "@/components/os-icon";
import { EmptyState, Page, PageHeader } from "@/components/page";
import { StateLabel } from "@/components/state-dot";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { connectionLabel, machineOs, machineSubtitle, osName, presenceLabel, presenceWord, reachable, selectedMachine, spaceCount } from "@/lib/machines";
import { osLabel, realSpaces, spaceState } from "@/lib/spaces";
import { cn, relativeTime } from "@/lib/utils";

export const Route = createFileRoute("/machines/")({ component: MachinesPage });

function MachinesPage() {
  const { data: machines = [], isLoading, error, setupGuide } = useMachines();
  const { data: spaces = [] } = useSpaces();
  const { core } = useBridge();
  const exp = useExperiments();
  const experiments = exp.unsupported ? undefined : (exp.data?.experiments ?? {});
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const selected = selectedMachine(machines, selectedId);
  // Reachable from here: a machine whose owner stopped sharing it is not.
  const online = machines.filter(reachable).length;
  // Another machine's own Space (`relay:<id>`), drawn by the core as the
  // SwiftUI detail draws it: its word when it can't be reached, and Remove.
  const access = machines.find((m) => m.current)?.accessNotice ?? null;
  const detailOf = (m: Machine): { space: Space; detail: SpaceDetailView } | null => {
    if (m.current) return null;
    const space = spaces.find((s) => s.id === `relay:${m.id}`);
    return space ? { space, detail: spaceDetail(core, space, null, null, experiments, access) } : null;
  };
  const reachWord = (m: Machine) => {
    const d = detailOf(m)?.detail;
    return d && !d.canStream && !d.desktopNote ? d.previewText : null;
  };

  return (
    <Page>
      <PageHeader
        title="Machines"
        description={
          isLoading ? "Loading machines…" : machines.length ? `${machines.length} machines, ${online} online` : "Computers that can run your Spaces."
        }
        actions={
          <Button variant="outline" onClick={() => setAdding(true)}>
            <PlusIcon /> Add a machine
          </Button>
        }
      />
      {error ? (
        <EmptyState icon={<ServerIcon />} title="Couldn't load machines">
          {error.message}
        </EmptyState>
      ) : isLoading ? (
        <LoadingState />
      ) : machines.length === 0 ? (
        <EmptyState
          icon={<ServerIcon />}
          title="No machines yet"
          action={
            <Button onClick={() => setAdding(true)}>
              <PlusIcon /> Add a machine
            </Button>
          }
        >
          Set up a Mac, Linux or Windows computer for access and it shows here, ready to run Spaces.
        </EmptyState>
      ) : (
        <div className="grid grid-cols-[minmax(0,340px)_minmax(0,1fr)] items-start gap-6">
          <div>
            <ul aria-label="Machines" className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">
              {machines.map((m) => (
                <li key={m.id}>
                  <MachineRow
                    machine={m}
                    reach={reachWord(m)}
                    spaces={realSpaces(spaces).filter((s) => m.spaceIds.includes(s.id)).length}
                    selected={m.id === selected?.id}
                    onSelect={() => setSelectedId(m.id)}
                  />
                </li>
              ))}
            </ul>
            {machines.length === 1 ? (
              <p className="mt-3 px-1 text-xs text-muted-foreground">
                Only this machine so far. Add another to run Spaces on it from here.
              </p>
            ) : null}
          </div>
          {selected ? (
            <MachineDetail
              key={selected.id}
              machine={selected}
              reach={reachWord(selected)}
              own={detailOf(selected)}
              spaces={realSpaces(spaces).filter((s) => selected.spaceIds.includes(s.id))}
              onRemoved={() => setSelectedId(null)}
            />
          ) : null}
        </div>
      )}
      <AddMachineDialog open={adding} onOpenChange={setAdding} guide={setupGuide} />
    </Page>
  );
}

function MachineIcon({ machine, className }: { machine: Machine; className?: string }) {
  const os = machineOs(machine);
  if (os) return <OsIcon os={os} className={className} />;
  const Icon = machine.current ? LaptopIcon : ServerIcon;
  return <Icon className={className} strokeWidth={1.75} />;
}

function PresenceDot({ online, notSharing }: { online: boolean; notSharing?: boolean }) {
  // Not reachable (its owner stopped sharing it): the quiet dot, as the
  // SwiftUI sidebar draws it.
  const tone = !online || notSharing ? "bg-muted-foreground/40" : "bg-success";
  return <span className={cn("size-1.5 shrink-0 rounded-full", tone)} aria-hidden />;
}

function MachineRow({ machine: m, reach, spaces, selected, onSelect }: { machine: Machine; reach: string | null; spaces: number; selected: boolean; onSelect: () => void }) {
  return (
    <button
      type="button"
      data-machine-id={m.id}
      data-online={m.online ? "true" : "false"}
      data-not-sharing={m.notSharing ? "true" : undefined}
      aria-current={selected ? "true" : undefined}
      onClick={onSelect}
      className={cn(
        "flex w-full cursor-default items-center gap-3 px-3.5 py-3 text-left outline-none transition-colors focus-visible:bg-accent",
        selected ? "bg-accent" : "hover:bg-accent/60",
      )}
    >
      <div className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
        <MachineIcon machine={m} className="size-4" />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span className="truncate text-[13px] font-medium">{m.name}</span>
          {m.current ? <span className="shrink-0 text-xs text-muted-foreground">(this machine)</span> : null}
        </div>
        <div className="truncate text-xs text-muted-foreground">{machineSubtitle(m)}</div>
      </div>
      <div className="flex shrink-0 flex-col items-end gap-1">
        <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
          <PresenceDot online={m.online} notSharing={Boolean(m.notSharing)} />
          {presenceWord(m, reach)}
        </span>
        <span className="text-xs text-muted-foreground tabular-nums">{spaceCount(spaces)}</span>
      </div>
    </button>
  );
}

function MachineDetail({
  machine: m,
  reach,
  own,
  spaces,
  onRemoved,
}: {
  machine: Machine;
  reach: string | null;
  own: { space: Space; detail: SpaceDetailView } | null;
  spaces: Space[];
  onRemoved: () => void;
}) {
  const now = Date.now();
  const { deleteSpace } = useSpaces();
  const [confirming, setConfirming] = useState(false);
  // Remove: the core's Delete action for the machine's own Space, as the
  // SwiftUI detail's toolbar has it.
  const remove = own?.detail.actions.find((a) => a.id === "delete") ?? null;
  const run = (removeOnly: boolean) => {
    if (!own) return;
    onRemoved();
    deleteSpace(own.space.id, removeOnly || own.detail.removeOnly).then(
      () => toast(`Removed ${m.name}`, { type: "success" }),
      (e: unknown) => toast(`Couldn't remove ${m.name}`, { description: e instanceof Error ? e.message : String(e), type: "error" }),
    );
  };
  return (
    <section aria-label={`${m.name} details`} className="min-w-0">
      <div className="mb-5 flex items-center gap-3">
        <div className="flex size-10 shrink-0 items-center justify-center rounded-xl bg-muted text-muted-foreground">
          <MachineIcon machine={m} className="size-5" />
        </div>
        <div className="min-w-0">
          <h2 className="truncate text-[15px] font-semibold">{m.name}</h2>
          <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <PresenceDot online={m.online} notSharing={Boolean(m.notSharing)} />
            {presenceLabel(m, now, reach)}
          </p>
        </div>
        {remove ? (
          <Button
            variant="outline"
            size="sm"
            className="ml-auto shrink-0"
            disabled={!remove.enabled}
            title={remove.help}
            onClick={() => setConfirming(true)}
            data-machine-remove={m.id}
          >
            <Trash2Icon className="text-destructive" /> {remove.label}
          </Button>
        ) : null}
      </div>
      {own ? (
        <ConfirmDialog
          open={confirming}
          onOpenChange={setConfirming}
          title={own.detail.confirm.title}
          description={own.detail.confirm.message}
          confirmLabel={own.detail.confirm.confirmLabel}
          cancelLabel={own.detail.confirm.cancelLabel}
          disabledReason={own.detail.confirm.confirmEnabled ? null : (own.detail.confirm.disabledReason ?? null)}
          destructive
          onConfirm={() => run(false)}
          alternative={own.detail.confirm.removeLabel ? { label: own.detail.confirm.removeLabel, onClick: () => run(true) } : null}
        />
      ) : null}

      <Group title="Overview">
        <Fact label="Operating system">{osName(m)}</Fact>
        {m.model ? <Fact label="Model">{m.model}</Fact> : null}
        <Fact label="Connection">{connectionLabel(m)}</Fact>
        {m.detail && !m.panel?.configured ? <Fact label="Status">{m.detail}</Fact> : null}
        {!m.online && m.lastSeen ? <Fact label="Last seen">{relativeTime(m.lastSeen * 1000, now)}</Fact> : null}
        {m.limits.map((l) => (
          <Fact key={l.resource} label="Limit" hint={l.reason}>
            {l.used} of {l.limit} in use
          </Fact>
        ))}
      </Group>

      <Group title="Spaces">
        {spaces.length === 0 ? (
          <p className="px-4 py-3 text-[13px] text-muted-foreground">
            {m.online ? "No Spaces on this machine." : "No Spaces listed. Spaces show again when the machine is back online."}
          </p>
        ) : (
          spaces.map((s) => (
            <div key={s.id} className="flex items-center gap-3 px-4 py-2.5">
              <OsIcon os={s.os} className="size-3.5 shrink-0 text-muted-foreground" />
              <div className="min-w-0 flex-1">
                <div className="truncate text-[13px]">{s.name}</div>
                <div className="truncate text-xs text-muted-foreground">{osLabel(s)}</div>
              </div>
              <StateLabel state={spaceState(s)} space={s} className="shrink-0" />
            </div>
          ))
        )}
      </Group>

      {m.panel ? <ThisMachine panel={m.panel} progress={m.hostProgress} /> : null}
    </section>
  );
}

function Group({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="mb-6">
      <h3 className="mb-2 px-1 text-xs font-semibold text-muted-foreground">{title}</h3>
      <div className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">{children}</div>
    </section>
  );
}

/** A label and its value. The label never shrinks; a long value (a
 * machine's status line) wraps instead of being cut. */
function Fact({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="flex min-h-10 items-start justify-between gap-6 px-4 py-2.5">
      <div className="max-w-[50%] shrink-0">
        <div className="text-[13px] whitespace-nowrap">{label}</div>
        {hint ? <div className="mt-0.5 text-xs text-muted-foreground">{hint}</div> : null}
      </div>
      <div data-fact-value="" className="min-w-0 text-right text-[13px] break-words text-muted-foreground select-text">
        {children}
      </div>
    </div>
  );
}

function LoadingState() {
  return (
    <div aria-busy="true" aria-label="Loading machines" className="grid grid-cols-[minmax(0,340px)_minmax(0,1fr)] items-start gap-6">
      <div className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">
        {[0, 1, 2].map((i) => (
          <div key={i} className="flex items-center gap-3 px-3.5 py-3">
            <div className="size-8 animate-pulse rounded-lg bg-muted" />
            <div className="flex-1 space-y-1.5">
              <div className="h-3 w-28 animate-pulse rounded bg-muted" />
              <div className="h-2.5 w-40 animate-pulse rounded bg-muted" />
            </div>
          </div>
        ))}
      </div>
      <div className="space-y-3">
        <div className="h-10 w-48 animate-pulse rounded-lg bg-muted" />
        <div className="h-40 animate-pulse rounded-xl bg-muted" />
      </div>
    </div>
  );
}
