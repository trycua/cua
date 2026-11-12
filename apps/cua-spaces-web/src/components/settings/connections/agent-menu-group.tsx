// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useState } from "react";

import { HarnessMark } from "@/components/agents/harness-mark";
import { SettingsGroup } from "@/components/settings-group";
import { GroupedRow } from "@/components/ui/grouped";
import { Switch } from "@/components/ui/switch";
import { toastError } from "@/components/ui/toast";
import { useT } from "@/i18n";
import { useAgentMenu } from "@/hooks/use-agent-menu";
import type { Connectors } from "@/hooks/use-connectors";

/**
 * Settings, Connections, Agent menu: one switch per coding app for whether
 * Home's agent menu lists it. A hidden app keeps its sign-ins and stays on the
 * Agents page and available by name. The last app shown cannot be hidden, and
 * the group says so. Every word and rule is the core's (`agentMenu.view`).
 */
export function AgentMenuGroup({ c }: { c: Connectors }) {
  const t = useT();
  const { view, choose } = useAgentMenu(c);
  const [refused, setRefused] = useState<string | null>(null);
  if (!view) return null;
  const note = refused ?? view.lastNote;
  const flip = (app: string, on: boolean) => {
    setRefused(null);
    void choose(app, on)
      .then((no) => setRefused(no))
      .catch(toastError(t("settings.connections.agent_menu.change_failed")));
  };
  return (
    <SettingsGroup
      title={view.title}
      data-agent-menu-group=""
      footer={
        <span data-agent-menu-note={note ? "" : undefined}>
          {view.help}
          {note ? <> {note}</> : null}
        </span>
      }
    >
      {view.rows.map((r) => (
        <GroupedRow key={r.app} data-agent-menu-row={r.app} data-shown={r.shown}>
          <HarnessMark agent={r.app} name={r.label} />
          <div className="min-w-0 flex-1">
            <div className="truncate text-[13px]">{r.label}</div>
            <p className="mt-px truncate text-xs text-muted-foreground">{r.status}</p>
          </div>
          <Switch
            aria-label={`${view.switchLabel}: ${r.label}`}
            data-agent-menu-switch={r.app}
            checked={r.shown}
            disabled={r.locked}
            onCheckedChange={(on) => flip(r.app, on)}
          />
        </GroupedRow>
      ))}
    </SettingsGroup>
  );
}
