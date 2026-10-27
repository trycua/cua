// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ChevronDownIcon, ChevronRightIcon, ChevronUpIcon } from "lucide-react";
import { lazy, Suspense, useEffect, useState } from "react";

import type { AgentSetupRow } from "@/bridge";
import { HarnessMark } from "@/components/agents/harness-mark";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { Snippet } from "@/components/settings/snippet";
import { Button } from "@/components/ui/button";
import { GroupedAction, GroupedRow } from "@/components/ui/grouped";
import { Segmented } from "@/components/ui/segmented";
import { Switch } from "@/components/ui/switch";
import { useBridge } from "@/bridge";
import type { ConnectedApps } from "@/hooks/use-connected-apps";
import { BY_HAND, getItUrl, missingApps, shownApps } from "@/lib/connected-apps";
import { cn } from "@/lib/utils";

// Its miniature and setup load with the card.
const DriverSettingsCard = lazy(() => import("@/components/volume/driver-card").then((m) => ({ default: m.DriverSettingsCard })));

/**
 * Settings, Connectors, "Coding apps": the coding apps on this computer,
 * one switch each for Cua being set up in it (with Background computer use),
 * and "Set up new apps automatically". Rows show only the app and its switch.
 * "Show all" opens the apps that aren't installed, Background computer use,
 * and a collapsed "Advanced" with each app's settings file and adding Cua by hand.
 */
export function CodingAppsSection({ apps, focusApp }: { apps: ConnectedApps; focusApp?: string }) {
  const { openExternal } = useExternal();
  const shown = shownApps(apps.rows ?? []);
  const missing = missingApps(apps.rows ?? []);
  const [all, setAll] = useState(() => Boolean(focusApp) && !shown.some((r) => r.agent === focusApp) && missing.some((r) => r.agent === focusApp));
  const [lit, setLit] = useState<string | null>(null);
  const ready = apps.rows !== null;

  // Coming from the Agents page's Connect: bring that app's row into view and mark it for a moment.
  useEffect(() => {
    if (!ready || !focusApp) return;
    const row = document.querySelector<HTMLElement>(`[data-connected-app="${focusApp}"], [data-connected-missing="${focusApp}"]`);
    if (!row) return;
    row.scrollIntoView?.({ block: "center" });
    setLit(focusApp);
    const t = setTimeout(() => setLit(null), 2000);
    return () => clearTimeout(t);
  }, [ready, focusApp, all]);

  if (!ready) return <div className="mb-8 h-48" aria-busy="true" />;

  return (
    <div data-connected-apps="">
      <SettingsGroup title="Coding apps">
        {shown.length === 0 ? (
          <GroupedRow className="text-muted-foreground" data-connected-none="">
            We didn't find a coding app yet. Install one and Cua will notice.
          </GroupedRow>
        ) : (
          shown.map((row) => <AppRow key={row.agent} row={row} apps={apps} lit={lit === row.agent} />)
        )}
        <SettingsRow
          label="Set up new apps automatically"
          description="When you install another coding app, Cua adds itself and tells you."
          htmlFor="connected-auto"
          control={<Switch id="connected-auto" data-connected-auto="" checked={apps.state.auto === true} onCheckedChange={apps.setAuto} />}
        />
        {all && missing.length > 0 ? (
          <>
            <GroupedRow className="min-h-9 bg-muted/40 py-2 text-xs text-muted-foreground">Not installed</GroupedRow>
            {missing.map((row) => {
              const url = getItUrl(row.agent);
              return (
                <GroupedRow key={row.agent} data-connected-missing={row.agent} className={cn("text-muted-foreground transition-colors", lit === row.agent && "bg-accent")}>
                  <HarnessMark agent={row.agent} name={row.name} />
                  <span className="min-w-0 flex-1 text-[13px]">{row.name}</span>
                  {url ? (
                    <button type="button" className="text-xs hover:text-foreground hover:underline" onClick={() => void openExternal(url)}>
                      Get it
                    </button>
                  ) : null}
                </GroupedRow>
              );
            })}
          </>
        ) : null}
        <GroupedAction data-connected-show-all="" aria-expanded={all} onClick={() => setAll(!all)}>
          {all ? "Show fewer" : "Show all"} {all ? <ChevronUpIcon className="size-3.5" /> : <ChevronDownIcon className="size-3.5" />}
        </GroupedAction>
      </SettingsGroup>
      {all ? (
        <>
          <SettingsGroup title="Background computer use">
            <Suspense fallback={<div className="h-48" />}>
              <DriverSettingsCard />
            </Suspense>
          </SettingsGroup>
          <Advanced rows={shown} />
        </>
      ) : null}
    </div>
  );
}

function useExternal() {
  const { data } = useBridge();
  return { openExternal: async (url: string) => void (await data?.call("session.openExternal", { url }).catch(() => {})) };
}

function AppRow({ row, apps, lit }: { row: AgentSetupRow; apps: ConnectedApps; lit?: boolean }) {
  const busy = apps.pending.includes(row.agent);
  const failure = apps.failures[row.agent];
  const on = row.configured && !failure;
  const justOff = !row.configured && apps.state.off.includes(row.agent);
  const note = failure ? null : justOff ? "Off." : null;
  return (
    <div data-connected-app={row.agent} data-on={String(on)} data-failed={failure ? "" : undefined} className={cn("transition-colors", failure && "bg-destructive/5", lit && "bg-accent")}>
      <GroupedRow>
        <HarnessMark agent={row.agent} name={row.name} />
        <div className="min-w-0 flex-1">
          <label htmlFor={`app-${row.agent}`} className="block truncate text-[13px]">
            {row.name}
          </label>
          {note ? <p className="mt-0.5 truncate text-xs text-muted-foreground">{note}</p> : null}
        </div>
        <Switch id={`app-${row.agent}`} checked={on} disabled={busy} onCheckedChange={(next) => void apps.toggle(row, next)} />
      </GroupedRow>
      {failure ? <Failure row={row} text={failure} busy={busy} onRetry={() => void apps.toggle(row, true)} /> : null}
    </div>
  );
}

/** A setup that didn't work: why, Try again, and the by-hand snippet for this app. */
function Failure({ row, text, busy, onRetry }: { row: AgentSetupRow; text: string; busy: boolean; onRetry: () => void }) {
  const [hand, setHand] = useState(false);
  const snippet = BY_HAND[row.agent];
  return (
    <div className="px-4 pb-4 pl-14" data-connected-failure={row.agent}>
      <p className="text-[13px] text-destructive">{text}</p>
      <div className="mt-3 flex gap-2">
        <Button size="sm" variant="outline" disabled={busy} onClick={onRetry}>
          Try again
        </Button>
        {snippet ? (
          <Button size="sm" variant="outline" aria-expanded={hand} onClick={() => setHand(!hand)}>
            Add it by hand
          </Button>
        ) : null}
      </div>
      {hand && snippet ? (
        <>
          <Snippet file={snippet.file} text={snippet.snippet} />
          <p className="mt-2 text-xs text-muted-foreground">Paste this into the file, then restart {row.name}.</p>
        </>
      ) : null}
    </div>
  );
}

/**
 * "Advanced", closed by default: the file each app keeps its settings in, and
 * adding Cua by hand. Most people never need either.
 */
function Advanced({ rows }: { rows: AgentSetupRow[] }) {
  const files = rows.filter((r) => r.mcpConfig);
  return (
    <details className="group mb-8" data-connected-advanced="">
      <summary className="-ml-1 flex w-fit cursor-default list-none items-center gap-1 rounded-lg px-1 py-0.5 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 [&::-webkit-details-marker]:hidden">
        <ChevronRightIcon className="size-3 transition-transform group-open:rotate-90" aria-hidden /> Advanced
      </summary>
      <div className="mt-8">
        {files.length > 0 ? (
          <SettingsGroup title="Settings files">
            {files.map((r) => (
              <GroupedRow key={r.agent} data-connected-file={r.agent}>
                <span className="w-40 shrink-0 truncate text-[13px]">{r.name}</span>
                <code className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground select-text" title={r.mcpConfig ?? undefined}>
                  {r.mcpConfig}
                </code>
              </GroupedRow>
            ))}
          </SettingsGroup>
        ) : null}
        <ByHand />
      </div>
    </details>
  );
}

/** "Add it by hand": for an app in an unusual place, another computer, or a work laptop where Cua may not edit settings. */
function ByHand() {
  const ids = Object.keys(BY_HAND);
  const [id, setId] = useState(ids[0]!);
  const entry = BY_HAND[id]!;
  const label: Record<string, string> = { "claude-code": "Claude Code", codex: "Codex", hermes: "Hermes", openclaw: "OpenClaw", cursor: "Cursor" };
  return (
    <SettingsGroup title="Add it by hand" className="mb-0" data-connected-by-hand="">
      <div className="px-4 py-4">
        <Segmented aria-label="App" value={id} options={ids.map((value) => ({ value, label: label[value] ?? value }))} onValueChange={setId} />
        <Snippet file={entry.file} text={entry.snippet} />
        <p className="mt-2 text-xs text-muted-foreground">
          Use the full path to <code className="font-mono">cua</code> if the app runs without your shell's PATH, as apps opened from the Dock do.
        </p>
      </div>
    </SettingsGroup>
  );
}
