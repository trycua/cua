// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

import { driverCopy, isUnsupported, useBridge, useDriverSetup, useVolumePins, type AgentSetupRow, type AgentSetupSummary } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { cn } from "@/lib/utils";
import { DriverPreview } from "./previews";

/**
 * The background computer-use card (`DriverCardView` in the SwiftUI app):
 * the core's miniature over one checkbox line, "cua-driver skill for
 * background computer-use". Ticked, setting up an agent also adds the
 * cua-driver skill and MCP server.
 */
export function DriverCard({
  checked,
  onCheckedChange,
  disabled,
  fixedMs,
}: {
  checked: boolean;
  onCheckedChange: (on: boolean) => void;
  disabled?: boolean;
  fixedMs?: number | "still";
}) {
  const { core } = useBridge();
  const copy = driverCopy(core);
  return (
    <div className="flex flex-col gap-2" data-driver-card="">
      <DriverPreview label={copy.agentsDriverImage} fixedMs={fixedMs} />
      <label className="flex items-center gap-2.5 text-[13px]">
        <Checkbox checked={checked} disabled={disabled} onCheckedChange={(on) => onCheckedChange(Boolean(on))} data-driver-toggle="" />
        <span className="truncate" data-driver-label="">
          {copy.agentsDriver}
        </span>
      </label>
    </div>
  );
}

/** One line per agent after a setup: the core's line, what changed, and what failed. */
export function DriverSummaries({ summaries }: { summaries: AgentSetupSummary[] }) {
  return (
    <ul className="flex flex-col gap-1.5" data-driver-summaries="">
      {summaries.map((s) => (
        <li key={s.line} className="min-w-0 text-[13px]" data-driver-summary={s.line} title={[s.text, ...s.failed].filter(Boolean).join("\n")}>
          <span className={cn("block truncate", s.failed.length > 0 && "text-destructive")} data-summary-line="">
            {s.line}
          </span>
          {s.text ? (
            <span className="block text-xs text-pretty text-muted-foreground" data-summary-text="">
              {s.text}
            </span>
          ) : null}
          {s.failed.map((f) => (
            <span key={f} className="block truncate text-xs text-destructive" data-summary-failed="">
              {f}
            </span>
          ))}
        </li>
      ))}
    </ul>
  );
}

/**
 * Settings, AI agents: the card, then "Set up" for every installed coding
 * agent and what it did. A host that can't run the setup says so.
 */
export function DriverSettingsCard() {
  const { data } = useBridge();
  const setup = useDriverSetup();
  const pinned = useVolumePins().driver;
  const [agents, setAgents] = useState<AgentSetupRow[] | null>(null);
  const [checked, setChecked] = useState(true);
  const [busy, setBusy] = useState(false);
  const [summaries, setSummaries] = useState<AgentSetupSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { core } = useBridge();
  const copy = driverCopy(core);

  useEffect(() => {
    if (!data) return;
    let live = true;
    data.call("agents.setup", {}).then(
      (rows) => live && setAgents(rows.filter((r) => r.installed)),
      () => live && setAgents([]),
    );
    return () => {
      live = false;
    };
  }, [data]);

  const run = async () => {
    if (!agents?.length) return;
    setBusy(true);
    setError(null);
    try {
      setSummaries(await setup.setUp(agents.map((a) => ({ id: a.agent, name: a.name }))));
    } catch (e) {
      setError(isUnsupported(e) ? "Setting up cua-driver needs the Cua Spaces app on this machine." : e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const shown = pinned?.summaries ?? summaries;
  return (
    <div className="flex flex-col gap-3 px-4 py-3.5">
      <DriverCard checked={checked} onCheckedChange={setChecked} disabled={busy} fixedMs={pinned?.tMs} />
      <div className="flex items-center justify-between gap-4">
        <p className="text-xs text-muted-foreground">
          {agents === null
            ? copy.agentsLooking
            : agents.length === 0
              ? copy.agentsNone
              : `For ${agents.map((a) => a.name).join(", ")}.`}
        </p>
        <Button variant="outline" size="sm" disabled={!checked || busy || !agents?.length} onClick={() => void run()}>
          {busy ? copy.agentsSettingUp : copy.agentsSetUp}
        </Button>
      </div>
      {error ? (
        <p className="text-xs text-destructive" role="alert">
          {error}
        </p>
      ) : null}
      {shown ? <DriverSummaries summaries={shown} /> : null}
    </div>
  );
}
