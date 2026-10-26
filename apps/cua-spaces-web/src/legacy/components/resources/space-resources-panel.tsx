// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useBridge, type ComputerResources, type SpaceResources } from "@/bridge";
import { lowDiskWarning, spacePanel, type DiskFitView, type SpacePanelInput, type SpacePanelView } from "@/lib/resources";
import { cn } from "@/lib/utils";

import { Bar, LowDiskWarning } from "@/legacy/components/resources/meters";

/**
 * A Space's numbers on its page: Processor, Memory and Space for files,
 * each a title, the main number, the one "Given ..." line and a bar, then
 * "How it fits on this Mac". The words are the core's
 * (`resources.spacePanel`); a TypeScript stand-in answers only without it.
 */
export function SpaceResourcesPanel({
  space,
  name,
  host,
  computer,
  spaces,
  baseName,
}: {
  space: SpaceResources;
  /** The Space's name ("macOS 1"). */
  name: string;
  /** Its computer's numbers, when it sent them. */
  host: ComputerResources | null;
  /** "this Mac" (`thisComputerText`), or the other computer's name. */
  computer: string;
  /** Every Space on that computer, for how it fits. */
  spaces: SpaceResources[];
  /** Its base image's system ("Ubuntu 24.04"), when known. */
  baseName: string | null;
}) {
  const { core } = useBridge();
  const input: SpacePanelInput = { space, name, host, computer, spaces, baseName };
  const view = core.tryCall<SpacePanelView>("resources.spacePanel", { input }) ?? spacePanel(input);
  return (
    <section data-space-resources="" aria-label="Resources" className="mt-6">
      <div className="grid gap-4 md:grid-cols-3">
        {view.cards.map((c) => (
          <div key={c.label} data-resource-card={c.label} className="rounded-xl border bg-card p-4 shadow-xs">
            <h3 className="text-xs text-muted-foreground">{c.label}</h3>
            <p className="mt-1.5 text-xl leading-tight font-semibold tracking-[-0.01em] tabular-nums">{c.value}</p>
            {c.given ? <p className="mt-1 text-xs text-muted-foreground">{c.given}</p> : null}
            {c.fraction != null ? <Bar fill={c.fraction} className="mt-3.5" /> : null}
          </div>
        ))}
      </div>
      {host && view.fit ? <DiskFit fit={view.fit} host={host} computer={computer} /> : null}
    </section>
  );
}

const SWATCH: Record<string, string> = {
  yours: "bg-muted-foreground/60",
  thisSpace: "bg-brand-strong",
  otherSpaces: "bg-brand",
  base: "bg-brand/50",
  free: "border bg-foreground/10",
};

/** "How it fits on this Mac": your own files, this Space, other Spaces, each shared base once, free. */
function DiskFit({ fit, host, computer }: { fit: DiskFitView; host: ComputerResources; computer: string }) {
  const warning = lowDiskWarning(host, computer.charAt(0).toUpperCase() + computer.slice(1));
  const bar = fit.parts.filter((p) => p.kind !== "free");
  return (
    <div data-disk-fit="" className="mt-4 rounded-xl border bg-card p-4 shadow-xs">
      <div className="flex items-baseline justify-between gap-3">
        <h3 className="text-[13px] font-semibold">How it fits on {computer}</h3>
        <span className="text-xs text-muted-foreground tabular-nums">{fit.total}</span>
      </div>
      <div className="mt-3 flex h-2 overflow-hidden rounded-full bg-foreground/10" role="img" aria-label={fit.parts.map((p) => `${p.label} ${p.value}`).join(", ")}>
        {bar.map((p) => (
          <div key={p.key} className={cn("h-full", SWATCH[p.kind])} style={{ width: `${p.fraction * 100}%` }} />
        ))}
      </div>
      <ul className="mt-3 flex flex-wrap gap-x-5 gap-y-1.5 text-xs">
        {fit.parts.map((p) => (
          <li key={p.key} data-disk-part={p.kind === "base" ? "base" : p.key} className="flex items-center gap-1.5 tabular-nums">
            <span className={cn("size-2 rounded-xs", SWATCH[p.kind])} aria-hidden />
            {p.label} · {p.value}
            {p.detail ? <span className="text-muted-foreground">, {p.detail}</span> : null}
          </li>
        ))}
      </ul>
      {fit.note ? <p data-disk-fit-note="" className="mt-3 text-xs text-muted-foreground">{fit.note}</p> : null}
      {warning ? <LowDiskWarning warning={warning} compact className="mt-4 rounded-lg border" /> : null}
    </div>
  );
}
