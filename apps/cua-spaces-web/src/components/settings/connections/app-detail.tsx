// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";
import { CheckIcon, ChevronLeftIcon, Loader2Icon } from "lucide-react";

import { HarnessMark } from "@/components/agents/harness-mark";
import { useThisAndOthers } from "@/components/agents/other-machine-block";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { Snippet } from "@/components/settings/snippet";
import { Button } from "@/components/ui/button";
import { DetailsDisclosure, GroupedRow, SettingRow } from "@/components/ui/grouped";
import { Switch } from "@/components/ui/switch";
import { useConnectors } from "@/hooks/use-connectors";
import { BY_HAND } from "@/lib/connected-apps";
import { cn } from "@/lib/utils";

/**
 * One coding app's page (Settings, Connectors, the row's chevron): Connect
 * while it isn't, the separate switches (Cua skills, Cua tools, Keep it set
 * up), which provider it signs in with and how, Sign in or Sign out, the
 * settings file, adding Cua by hand, and Remove. The core's
 * `connectors.appDetail`.
 */
export function ConnectorAppPage({ agent }: { agent: string }) {
  const c = useConnectors();
  const { hereName } = useThisAndOthers();
  const d = c.detail(agent);
  const back = (
    <Link to="/settings/connections" className="-ml-1 mb-4 inline-flex items-center gap-0.5 rounded-lg px-1 py-0.5 text-[13px] text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60" data-connector-back="">
      <ChevronLeftIcon className="size-4" /> {d?.backLabel ?? "Connectors"}
    </Link>
  );
  if (c.apps.rows === null) return <div className="h-96" aria-busy="true" />;
  if (!d) {
    return (
      <div>
        {back}
        <p className="text-[13px] text-muted-foreground">This app isn't on this machine.</p>
      </div>
    );
  }
  const busy = c.input.busy.some((b) => b.id === `app:${agent}`);
  const failure = c.input.failures.find((f) => f.id === `app:${agent}`)?.line;
  const snippet = BY_HAND[agent];
  return (
    <div data-connector-detail={agent}>
      {back}
      <header className="mb-6 flex items-center gap-3">
        <HarnessMark agent={d.agent} name={d.name} large />
        <div className="min-w-0 flex-1">
          {/* design-ok: the app's own page needs its name as its title; groups below use SettingsGroup */}
          <h2 className="truncate text-[17px] font-semibold">{d.name}</h2>
          <p className={cn("flex items-center gap-1 text-[13px]", d.connected ? "text-foreground" : "text-muted-foreground")} data-connector-detail-status="">
            {busy ? <Loader2Icon className="size-3.5 animate-spin" /> : d.connected ? <CheckIcon className="size-3.5 text-success" /> : null}
            {d.status}
          </p>
          {/* Connect edits the apps of this machine only: the page says which. */}
          <p className="text-xs text-muted-foreground" data-connector-on="">
            On {hereName}
          </p>
          {failure ? <p className="text-[13px] text-destructive">{failure}</p> : null}
        </div>
        {d.connect ? (
          <Button variant="outline" disabled={!d.connect.enabled || busy} onClick={() => void c.connectApp(agent)} data-connector-detail-connect="">
            {d.connect.label}
          </Button>
        ) : null}
      </header>

      {d.toggles.length ? (
      <SettingsGroup title={d.toolsTitle}>
        {d.toggles.map((t) => (
          <SettingsRow
            key={t.id}
            label={t.label}
            description={t.help}
            htmlFor={`connector-${t.id}`}
            control={<Switch id={`connector-${t.id}`} data-connector-toggle={t.id} checked={t.on} disabled={!t.enabled} onCheckedChange={(on) => void c.setToggle(agent, t.id, on)} />}
          />
        ))}
      </SettingsGroup>
      ) : null}

      <SettingsGroup title={d.signInTitle} footer={d.signIn?.way}>
        {d.signIn ? (
          <GroupedRow className="min-h-12" data-connector-detail-signin={d.signIn.provider}>
            <div className="min-w-0 flex-1">
              <div>{d.signIn.title}</div>
              <div className={cn("flex items-center gap-1 text-xs", d.signIn.connected ? "text-foreground" : "text-muted-foreground")}>
                {d.signIn.connected ? <CheckIcon className="size-3 text-success" /> : null}
                {d.signIn.status}
              </div>
            </div>
            <Button
              size="sm"
              variant={d.signIn.connected ? "ghost" : "outline"}
              disabled={!d.signIn.button.enabled}
              data-connector-detail-signin-action={d.signIn.button.id}
              onClick={() => (d.signIn!.button.id === "sign-out" ? void c.signOut(d.signIn!.provider) : c.openProvider(d.signIn!.provider, agent))}
            >
              {d.signIn.button.label}
            </Button>
          </GroupedRow>
        ) : (
          <GroupedRow className="text-muted-foreground">{d.signInNote}</GroupedRow>
        )}
      </SettingsGroup>

      {d.settingsFile || snippet ? (
        <DetailsDisclosure title="Advanced" className="mb-8">
          <div className="mt-4">
            {d.settingsFile ? (
              <SettingsGroup title={d.settingsFileLabel}>
                <SettingRow label={<code className="font-mono text-xs text-muted-foreground select-text">{d.settingsFile}</code>} />
              </SettingsGroup>
            ) : null}
            {snippet ? (
              <SettingsGroup title="Add it by hand" className="mb-0">
                <div className="px-4 py-4">
                  <Snippet file={snippet.file} text={snippet.snippet} />
                  <p className="mt-2 text-xs text-muted-foreground">Paste this into the file, then restart {d.name}.</p>
                </div>
              </SettingsGroup>
            ) : null}
          </div>
        </DetailsDisclosure>
      ) : null}

      {d.removeLabel ? (
        <Button variant="ghost" className="text-destructive" disabled={busy} onClick={() => void c.removeFromApp(agent)} data-connector-remove="">
          {d.removeLabel}
        </Button>
      ) : null}
    </div>
  );
}
