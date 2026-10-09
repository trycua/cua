// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";
import { HardDriveIcon } from "lucide-react";
import type { ReactNode } from "react";

import { useExperimentFlags, useVolume, useVolumePins, type ConflictView, type DriveAction, type LineView } from "@/bridge";
import { EmptyState, Page, PageHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { volumeUnavailableNote } from "@/lib/volume-notes";

export const Route = createFileRoute("/volume/")({ component: VolumePage });

/**
 * Volume, as the SwiftUI app's `DrivePageView` lays it out: Open in Finder
 * and where it is mounted, the access agents ask for, the grants in force,
 * sync per device and conflicts. Not a file browser: Finder is. Every line
 * and label is the core's (`drive.view`); shown only with the Cua Volume
 * experiment on.
 */
function VolumePage() {
  const experiments = useExperimentFlags();
  const pinned = Boolean(useVolumePins().volume);
  if (!pinned && experiments && !experiments.cuaVolume) {
    return (
      <Page className="max-w-3xl">
        <PageHeader title="Volume" />
        <EmptyState icon={<HardDriveIcon />} title="Cua Volume is off">
          It's an experiment for now. Turn it on in Settings, Experiments.
        </EmptyState>
      </Page>
    );
  }
  return <VolumeBody />;
}

function VolumeBody() {
  const { view, unsupported, act, spaceErrors } = useVolume();

  if (!view) return <div className="h-full" aria-busy="true" />;
  if (unsupported) {
    return (
      <Page className="max-w-3xl">
        <PageHeader title={view.title} />
        <EmptyState icon={<HardDriveIcon />} title="Not available here">
          This window can't reach Cua Volume yet. Open it from the Cua Spaces app.
        </EmptyState>
      </Page>
    );
  }

  const send = (a: DriveAction) => act(a);
  return (
    <Page className="max-w-3xl">
      <PageHeader title={view.title} />
      <div data-drive-page="" data-busy={view.busy ? "true" : undefined}>
        {view.openLabel ? (
          <Section>
            <div className="flex items-center gap-4 px-4 py-3">
              <p className="min-w-0 flex-1 truncate text-[13px] text-muted-foreground" title={view.mountPath ?? view.mountLine ?? undefined} data-drive-mount-line="">
                {view.mountLine}
              </p>
              <Button
                disabled={view.busy}
                data-drive-open=""
                data-path={view.mountPath ?? ""}
                onClick={() => send({ type: "open-volume", mounted: view.mountPath ?? null })}
              >
                {view.openLabel}
              </Button>
            </div>
          </Section>
        ) : null}

        {view.requests.length > 0 ? (
          <Section title={view.requestsTitle} kind="requests">
            {view.requests.map((l) => (
              <Line
                key={l.id}
                line={l}
                busy={view.busy}
                onAction={() => send({ type: "approve", id: l.id })}
                onSecondary={() => send({ type: "deny", id: l.id })}
                primary
              />
            ))}
          </Section>
        ) : null}

        <Section title={view.grantsTitle} kind="grants">
          {view.grants.length === 0 ? (
            <p className="px-4 py-3 text-[13px] text-muted-foreground" data-drive-grants-empty="">
              {view.grantsEmpty}
            </p>
          ) : (
            view.grants.map((l) => <Line key={l.id} line={l} busy={view.busy} onAction={() => send({ type: "revoke", id: l.id })} />)
          )}
        </Section>

        {spaceErrors.length > 0 ? (
          <Section title="Spaces without Cua Volume" kind="space-errors">
            {spaceErrors.map((e) => (
              <div key={e.space} className="px-4 py-2.5" data-drive-space-error={e.space}>
                <p className="truncate text-[13px] font-medium">{e.space}</p>
                <p className="mt-0.5 text-xs text-muted-foreground" title={e.error}>
                  {volumeUnavailableNote(e.error)}
                </p>
              </div>
            ))}
          </Section>
        ) : null}

        {view.devices.length > 0 || view.syncNote ? (
          <Section title={view.devicesTitle} kind="devices">
            {view.devices.map((l) => (
              <Line key={l.id} line={l} busy={view.busy} />
            ))}
            {view.syncNote ? (
              <p
                className={cn("truncate px-4 py-2.5 text-xs", view.syncError ? "text-destructive" : "text-muted-foreground")}
                title={view.syncNote}
                data-drive-sync-note=""
                data-error={view.syncError ? "true" : "false"}
              >
                {view.syncNote}
              </p>
            ) : null}
          </Section>
        ) : null}

        {view.conflicts.length > 0 ? (
          <Section title={view.conflictsTitle} kind="conflicts">
            {view.conflicts.map((c) => (
              <Conflict
                key={c.path}
                conflict={c}
                busy={view.busy}
                onOpen={(path) => send({ type: "reveal", path })}
                onResolve={() => send({ type: "resolve", path: c.path })}
              />
            ))}
          </Section>
        ) : null}

        {view.error ? (
          <p className="px-1 text-[13px] text-destructive" role="alert" data-drive-error="">
            {view.error}
          </p>
        ) : null}
      </div>
    </Page>
  );
}

function Section({ title, kind, children }: { title?: string; kind?: string; children: ReactNode }) {
  return (
    <section className="mb-6" data-drive-section={kind}>
      {title ? (
        <h2 className="mb-2 px-1 text-xs font-semibold text-muted-foreground" data-drive-section-title="">
          {title}
        </h2>
      ) : null}
      <div className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">{children}</div>
    </section>
  );
}

/** A `LineView`: its text, a muted trailing part, and up to two buttons. */
function Line({ line, busy, onAction, onSecondary, primary }: { line: LineView; busy: boolean; onAction?: () => void; onSecondary?: () => void; primary?: boolean }) {
  return (
    <div className="flex min-h-11 items-center gap-4 px-4 py-2" data-drive-line={line.id}>
      <span className="min-w-0 flex-1 truncate text-[13px]" title={line.text} data-line-text="">
        {line.text}
      </span>
      {line.trailing ? (
        <span className="max-w-[40%] shrink-0 truncate text-xs text-muted-foreground" title={line.trailing} data-line-trailing="">
          {line.trailing}
        </span>
      ) : null}
      {line.secondaryLabel && onSecondary ? (
        <Button variant="ghost" size="sm" disabled={busy} onClick={onSecondary} data-line-secondary="">
          {line.secondaryLabel}
        </Button>
      ) : null}
      {line.actionLabel && onAction ? (
        <Button variant={primary ? "default" : "outline"} size="sm" disabled={busy} onClick={onAction} data-line-action="">
          {line.actionLabel}
        </Button>
      ) : null}
    </div>
  );
}

function Conflict({ conflict: c, busy, onOpen, onResolve }: { conflict: ConflictView; busy: boolean; onOpen: (path: string) => void; onResolve: () => void }) {
  return (
    <div className="flex min-h-11 items-center gap-3 px-4 py-2" data-drive-conflict={c.path}>
      <span className="min-w-0 flex-1 truncate text-[13px]" title={c.text} data-line-text="">
        {c.text}
      </span>
      <span className="shrink-0 truncate text-xs text-muted-foreground" data-line-trailing="">
        {c.trailing}
      </span>
      {c.openLabel && c.reveal ? (
        <Button variant="ghost" size="sm" disabled={busy} title={c.reveal} data-conflict-open={c.reveal} onClick={() => onOpen(c.reveal!)}>
          {c.openLabel}
        </Button>
      ) : null}
      <Button variant="outline" size="sm" disabled={busy} onClick={onResolve} data-conflict-resolve="">
        {c.resolveLabel}
      </Button>
    </div>
  );
}
