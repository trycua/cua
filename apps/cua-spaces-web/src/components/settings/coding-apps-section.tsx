// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ChevronDownIcon, ChevronUpIcon } from "lucide-react";
import { lazy, Suspense, useState } from "react";

import type { AgentSetupRow } from "@/bridge";
import { HarnessMark } from "@/components/agents/harness-mark";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { Snippet } from "@/components/settings/snippet";
import { Button } from "@/components/ui/button";
import { Segmented } from "@/components/ui/segmented";
import { Switch } from "@/components/ui/switch";
import { useBridge } from "@/bridge";
import { useConnectedApps } from "@/hooks/use-connected-apps";
import { BY_HAND, getItUrl, missingApps, shownApps } from "@/lib/connected-apps";
import { cn } from "@/lib/utils";

// Its miniature and setup load with the card.
const DriverSettingsCard = lazy(() => import("@/components/volume/driver-card").then((m) => ({ default: m.DriverSettingsCard })));

/**
 * Settings, Connected apps, "Coding apps": the coding apps on this computer,
 * one switch each for Cua being set up in it (with Background computer use),
 * and "Set up new apps automatically". "Show all" opens file paths, the apps
 * that aren't installed, and adding Cua by hand.
 */
export function CodingAppsSection() {
  const apps = useConnectedApps();
  const { openExternal } = useExternal();
  const [all, setAll] = useState(false);
  const shown = shownApps(apps.rows ?? []);
  const missing = missingApps(apps.rows ?? []);

  if (apps.rows === null) return <div className="mb-8 h-48" aria-busy="true" />;

  return (
    <div data-connected-apps="">
      <SettingsGroup title="Coding apps">
        {shown.length === 0 ? (
          <div className="px-4 py-3 text-[13px] text-muted-foreground" data-connected-none="">
            We didn't find a coding app yet. Install one and Cua will notice.
          </div>
        ) : (
          shown.map((row) => <AppRow key={row.agent} row={row} apps={apps} showPath={all} />)
        )}
        <SettingsRow
          label="Set up new apps automatically"
          description="When you install another coding app, Cua adds itself and tells you."
          htmlFor="connected-auto"
          control={<Switch id="connected-auto" data-connected-auto="" checked={apps.state.auto === true} onCheckedChange={apps.setAuto} />}
        />
        {all && missing.length > 0 ? (
          <>
            <div className="bg-muted/40 px-4 py-3 text-xs text-muted-foreground">Not installed</div>
            {missing.map((row) => {
              const url = getItUrl(row.agent);
              return (
                <div key={row.agent} data-connected-missing={row.agent} className="flex min-h-12 items-center gap-3 px-4 py-2.5 text-muted-foreground">
                  <HarnessMark agent={row.agent} name={row.name} />
                  <span className="min-w-0 flex-1 text-[13px]">{row.name}</span>
                  {url ? (
                    <button type="button" className="text-xs hover:text-foreground hover:underline" onClick={() => void openExternal(url)}>
                      Get it
                    </button>
                  ) : null}
                </div>
              );
            })}
          </>
        ) : null}
      </SettingsGroup>
      <button
        type="button"
        data-connected-show-all=""
        aria-expanded={all}
        onClick={() => setAll(!all)}
        className="-mt-4 mb-8 flex items-center gap-1 px-1 text-[13px] text-muted-foreground outline-none hover:text-foreground focus-visible:text-foreground"
      >
        {all ? "Show fewer" : "Show all"} {all ? <ChevronUpIcon className="size-3.5" /> : <ChevronDownIcon className="size-3.5" />}
      </button>
      {all ? (
        <>
          <ByHand />
          <SettingsGroup title="Background computer use">
            <Suspense fallback={<div className="h-48" />}>
              <DriverSettingsCard />
            </Suspense>
          </SettingsGroup>
        </>
      ) : null}
    </div>
  );
}

function useExternal() {
  const { data } = useBridge();
  return { openExternal: async (url: string) => void (await data?.call("session.openExternal", { url }).catch(() => {})) };
}

function AppRow({ row, apps, showPath }: { row: AgentSetupRow; apps: ReturnType<typeof useConnectedApps>; showPath: boolean }) {
  const busy = apps.pending.includes(row.agent);
  const failure = apps.failures[row.agent];
  const on = row.configured && !failure;
  const justOff = !row.configured && apps.state.off.includes(row.agent);
  const note = failure ? null : justOff ? "Off." : showPath && row.mcpConfig ? row.mcpConfig : null;
  return (
    <div data-connected-app={row.agent} data-on={String(on)} data-failed={failure ? "" : undefined} className={cn(failure && "bg-destructive/5")}>
      <div className="flex min-h-12 items-center gap-3 px-4 py-2.5">
        <HarnessMark agent={row.agent} name={row.name} />
        <div className="min-w-0 flex-1">
          <label htmlFor={`app-${row.agent}`} className="block truncate text-[13px]">
            {row.name}
          </label>
          {note ? <p className={cn("mt-0.5 truncate text-xs text-muted-foreground", showPath && !justOff && "font-mono")}>{note}</p> : null}
        </div>
        <Switch id={`app-${row.agent}`} checked={on} disabled={busy} onCheckedChange={(next) => void apps.toggle(row, next)} />
      </div>
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
      {row.mcpConfig ? <p className="-mt-1 mb-1 font-mono text-xs text-muted-foreground">{row.mcpConfig}</p> : null}
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

/** "Add it by hand": for an app in an unusual place, another computer, or a work laptop where Cua may not edit settings. */
function ByHand() {
  const ids = Object.keys(BY_HAND);
  const [id, setId] = useState(ids[0]!);
  const entry = BY_HAND[id]!;
  const label: Record<string, string> = { "claude-code": "Claude Code", codex: "Codex", hermes: "Hermes", openclaw: "OpenClaw", cursor: "Cursor" };
  return (
    <section className="mb-8" data-connected-by-hand="">
      <h2 className="mb-2 px-1 text-xs font-semibold text-muted-foreground">Add it by hand</h2>
      <div className="rounded-xl border bg-card p-4 shadow-xs">
        <Segmented aria-label="App" value={id} options={ids.map((value) => ({ value, label: label[value] ?? value }))} onValueChange={setId} />
        <Snippet file={entry.file} text={entry.snippet} />
        <p className="mt-2 text-xs text-muted-foreground">
          Use the full path to <code className="font-mono">cua</code> if the app runs without your shell's PATH, as apps opened from the Dock do.
        </p>
      </div>
    </section>
  );
}
