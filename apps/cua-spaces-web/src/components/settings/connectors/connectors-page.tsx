// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";
import { CheckIcon, ChevronDownIcon, ChevronRightIcon, ChevronUpIcon, Loader2Icon, TriangleAlertIcon } from "lucide-react";
import { lazy, Suspense, useEffect, useState, type KeyboardEvent, type MouseEvent } from "react";

import { useAgentKeys, useBridge, useKeysRecheck, type ConnectorAppRow, type ConnectorProviderRow } from "@/bridge";
import { OtherMachineBlock, useThisAndOthers } from "@/components/agents/other-machine-block";
import { HarnessMark } from "@/components/agents/harness-mark";
import { KeySheet } from "@/components/agents/key-sheet";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { GroupedAction, GroupedRow } from "@/components/ui/grouped";
import { Switch } from "@/components/ui/switch";
import { toastError } from "@/components/ui/toast";
import { useConnectors, type Connectors } from "@/hooks/use-connectors";
import { addedText } from "@/lib/agent-keys";
import { getItUrl } from "@/lib/connected-apps";
import { cn } from "@/lib/utils";

const DriverSettingsCard = lazy(() => import("@/components/volume/driver-card").then((m) => ({ default: m.DriverSettingsCard })));

/**
 * Settings, Connectors: Coding apps, each with Connect (Cua's
 * tools and the app's sign-in, once) and a chevron to its own page; then
 * Providers, each with Connect (the provider's sign-in where there is one
 * for this, else a key) or Sign out. Every word and state is the core's
 * (`connectors.view`).
 */
export function ConnectorsRedesign({ focusApp }: { focusApp?: string }) {
  const c = useConnectors();
  const { view } = c;
  const navigate = useNavigate();
  const { data } = useBridge();
  const keys = useAgentKeys();
  // Opening this page asks the providers whether the saved sign-ins and keys still work.
  useKeysRecheck();
  const [all, setAll] = useState(false);
  const [otherOpen, setOtherOpen] = useState(false);
  const { hereName, others } = useThisAndOthers();
  const [lit, setLit] = useState<string | null>(null);

  useEffect(() => {
    if (!view || !focusApp) return;
    document.querySelector<HTMLElement>(`[data-connector-app="${focusApp}"]`)?.scrollIntoView?.({ block: "center" });
    setLit(focusApp);
    const t = setTimeout(() => setLit(null), 2000);
    return () => clearTimeout(t);
  }, [Boolean(view), focusApp]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!view || c.apps.rows === null) return <div className="mb-8 h-48" aria-busy="true" />;
  const open = (agent: string) => void navigate({ to: "/settings/connectors/$app", params: { app: agent } });
  const confirm = keys.removing ? keys.removeConfirmOf(keys.removing) : null;

  return (
    <div data-connectors-redesign="">
      {view.notice ? (
        <div data-connectors-notice="" className="mb-4 flex items-start gap-2.5 rounded-lg bg-warning/10 px-4 py-2.5 text-[13px]">
          <TriangleAlertIcon className="mt-0.5 size-4 shrink-0 text-warning" />
          <span className="min-w-0 flex-1">{view.notice}</span>
        </div>
      ) : null}

      <SettingsGroup
        title={view.appsTitle}
        data-connectors-apps=""
        footer={
          // Connect edits the coding apps of this machine only: it says which, and what it leaves alone.
          <span data-connectors-machine="">
            On {hereName}.{others.length > 0 ? ` Not touched: ${others.join(", ")}. An agent on one of those machines needs its own setup (see On another machine).` : ""}
          </span>
        }
      >
        {view.appsEmpty ? <GroupedRow className="text-muted-foreground">{view.appsEmpty}</GroupedRow> : null}
        {view.apps.map((row) => (
          <AppRow key={row.agent} row={row} lit={lit === row.agent} onOpen={() => open(row.agent)} onConnect={() => void c.connectApp(row.agent)} />
        ))}
        <SettingsRow
          label={view.autoLabel}
          description={view.autoHelp}
          htmlFor="connectors-auto"
          control={<Switch id="connectors-auto" data-connected-auto="" checked={view.auto} onCheckedChange={c.apps.setAuto} />}
        />
        {all && (view?.more.length ?? 0) > 0 ? (
          <>
            <GroupedRow className="min-h-9 bg-muted/40 py-2 text-xs text-muted-foreground">Not installed</GroupedRow>
            {view!.more.map((row) => {
              const url = getItUrl(row.agent);
              return (
                <GroupedRow key={row.agent} data-connected-missing={row.agent} className="text-muted-foreground">
                  <HarnessMark agent={row.agent} name={row.name} />
                  <span className="min-w-0 flex-1 text-[13px]">{row.name}</span>
                  {url ? (
                    <button type="button" className="text-xs hover:text-foreground hover:underline" onClick={() => void data?.call("session.openExternal", { url }).catch(() => {})}>
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
        <SettingsGroup title="Background computer use">
          <Suspense fallback={<div className="h-48" />}>
            <DriverSettingsCard />
          </Suspense>
        </SettingsGroup>
      ) : null}

      <SettingsGroup title="On another machine" data-connectors-other-machine-group="">
        <GroupedRow>
          <div className="min-w-0 flex-1">
            <div className="text-[13px]">An agent on another machine</div>
            <div className="text-xs text-muted-foreground">Use {hereName} from an agent that runs somewhere else.</div>
          </div>
          <Button size="sm" variant="outline" aria-expanded={otherOpen} onClick={() => setOtherOpen(!otherOpen)} data-connectors-other-toggle="">
            {otherOpen ? "Hide" : "Show how"}
          </Button>
        </GroupedRow>
        {otherOpen ? <OtherMachineBlock /> : null}
      </SettingsGroup>

      <SettingsGroup title={view.providersTitle} footer={view.providersFooter} data-connectors-providers="">
        {view.providers.map((p) => (
          <ProviderRow key={p.id} row={p} addedLabel={view.addedLabel} c={c} />
        ))}
        {view.others.map((r) => (
          <GroupedRow key={r.env} data-agent-key-row={r.env}>
            <div className="min-w-0 flex-1">
              <div className="truncate font-mono text-[12px]">{r.title}</div>
              <div className="truncate text-xs text-muted-foreground">{r.detail}</div>
            </div>
            <span className="font-mono text-[12px] text-muted-foreground">{r.status}</span>
            <Button size="sm" variant="outline" disabled={!view.canEdit} onClick={() => keys.openSheet("other", r.env)}>
              {r.actionLabel}
            </Button>
            {r.removeLabel ? (
              <Button size="sm" variant="ghost" disabled={!view.canEdit} onClick={() => keys.askRemove(r.env)}>
                {r.removeLabel}
              </Button>
            ) : null}
          </GroupedRow>
        ))}
        <SettingsRow
          label={view.addOtherLabel}
          description={view.otherHelp}
          control={
            <Button size="sm" variant="outline" disabled={!view.canEdit} onClick={() => keys.openSheet("other")} data-agent-key-add-other="">
              Add key
            </Button>
          }
        />
      </SettingsGroup>

      {keys.sheet ? <KeySheet key={`${keys.sheet.provider}:${keys.sheet.env ?? ""}`} keys={keys} provider={keys.sheet.provider} env={keys.sheet.env} /> : null}
      <ConfirmDialog
        open={confirm !== null}
        onOpenChange={(o) => !o && keys.askRemove(null)}
        title={confirm?.title ?? ""}
        description={confirm?.message ?? ""}
        confirmLabel={confirm?.confirmLabel ?? "Remove"}
        cancelLabel={confirm?.cancelLabel}
        destructive
        onConfirm={() => {
          const env = keys.removing;
          if (env) void keys.remove(env).catch(toastError("Couldn't remove the key"));
        }}
      />
    </div>
  );
}

/** A coding app: the whole row opens its page (hover, chevron); Connect works on its own. */
function AppRow({ row, lit, onOpen, onConnect }: { row: ConnectorAppRow; lit: boolean; onOpen: () => void; onConnect: () => void }) {
  const stop = (e: MouseEvent | KeyboardEvent) => e.stopPropagation();
  return (
    <div
      role="link"
      tabIndex={0}
      aria-label={row.openLabel}
      data-connector-app={row.agent}
      data-state={row.state}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen();
        }
      }}
      className={cn(
        "flex min-h-12 cursor-default items-center gap-3 px-4 py-2.5 text-[13px] outline-none transition-colors hover:bg-accent/50 focus-visible:bg-accent/50",
        row.state === "failed" && "bg-destructive/5",
        lit && "bg-accent",
      )}
    >
      <HarnessMark agent={row.agent} name={row.name} />
      <div className="min-w-0 flex-1">
        <div className="truncate">{row.name}</div>
        {row.line ? (
          <p className={cn("mt-px truncate text-xs", row.state === "failed" ? "text-destructive" : "text-muted-foreground")} data-connector-line="">
            {row.line}
          </p>
        ) : null}
      </div>
      {row.connectedLabel ? (
        <span className="flex items-center gap-1 text-[13px] text-muted-foreground" data-connector-connected="">
          <CheckIcon className="size-3.5 text-success" /> {row.connectedLabel}
        </span>
      ) : null}
      {row.button ? (
        <Button size="sm" variant="outline" disabled={!row.button.enabled} data-connector-connect={row.button.id} onClick={(e) => (stop(e), onConnect())} onKeyDown={stop}>
          {row.state === "connecting" ? <Loader2Icon className="animate-spin" /> : null}
          {row.button.label}
        </Button>
      ) : null}
      <ChevronRightIcon aria-hidden className="size-4 shrink-0 text-muted-foreground/70" data-connector-chevron="" />
    </div>
  );
}

function ProviderRow({ row, addedLabel, c }: { row: ConnectorProviderRow; addedLabel: string; c: Connectors }) {
  const connected = row.state === "connected";
  return (
    <GroupedRow data-connector-provider={row.id} data-state={row.state} className="min-h-12">
      <div className="min-w-0 flex-1">
        <div className="truncate">{row.label}</div>
        <div className="truncate text-xs text-muted-foreground">{row.detail}</div>
      </div>
      <div className="flex shrink-0 flex-col items-end">
        <span className={cn("flex items-center gap-1.5 text-[13px]", connected ? "" : row.state === "failed" ? "text-destructive" : "text-muted-foreground")} data-connector-status="">
          {row.state === "connecting" ? <Loader2Icon className="size-3.5 animate-spin" /> : connected ? <CheckIcon className="size-3.5 text-success" /> : null}
          {row.status}
        </span>
        {row.addedMs !== null ? <span className="text-[11px] text-muted-foreground">{addedText(addedLabel, row.addedMs)}</span> : null}
      </div>
      <Button
        size="sm"
        variant={connected ? "ghost" : "outline"}
        disabled={!row.button.enabled}
        data-connector-provider-action={row.button.id}
        onClick={() => (row.button.id === "sign-out" ? void c.signOut(row.id) : c.openProvider(row.id))}
      >
        {row.button.label}
      </Button>
    </GroupedRow>
  );
}
