// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";
import { CheckIcon, ChevronDownIcon, ChevronRightIcon, ChevronUpIcon, Loader2Icon, TriangleAlertIcon } from "lucide-react";
import { lazy, Suspense, useEffect, useState, type KeyboardEvent, type MouseEvent } from "react";

import { useAgentKeys, useBridge, useKeysRecheck, type AgentKeysHook, type ConnectorAppRow, type ConnectorProviderRow } from "@/bridge";
import { OtherMachineBlock, useThisAndOthers } from "@/components/agents/other-machine-block";
import { HarnessMark } from "@/components/agents/harness-mark";
import { KeySheet } from "@/components/agents/key-sheet";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { AgentMenuGroup } from "@/components/settings/connections/agent-menu-group";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Dialog, DialogDescription, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { FactRow, GroupedAction, GroupedRow } from "@/components/ui/grouped";
import { Input } from "@/components/ui/input";
import { GithubGroup } from "@/components/settings/connections/github-group";
import { ProviderMark } from "@/components/settings/connections/provider-mark";
import { Switch } from "@/components/ui/switch";
import { toastError } from "@/components/ui/toast";
import { useConnectors, type Connectors } from "@/hooks/use-connectors";
import { addedText } from "@/lib/agent-keys";
import { getItUrl } from "@/lib/connected-apps";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";

const DriverSettingsCard = lazy(() => import("@/components/volume/driver-card").then((m) => ({ default: m.DriverSettingsCard })));

/**
 * Settings, Connections: Coding apps, each with Connect (Cua's tools and the
 * app's sign-in, once) and a chevron to its own page; On another machine; then
 * Providers, drawn the same way (an icon, the name, one grey line, Connect or
 * a check, a chevron to a sheet with the apps it works with, when it was added
 * and Disconnect), with Show all for the rest. Every word and state is the
 * core's (`connectors.view`).
 */
export function ConnectorsRedesign({ focusApp }: { focusApp?: string }) {
  const t = useT();
  const c = useConnectors();
  const { view } = c;
  const navigate = useNavigate();
  const { data } = useBridge();
  const keys = useAgentKeys();
  // Opening this page asks the providers whether the saved sign-ins and keys still work.
  useKeysRecheck();
  const [all, setAll] = useState(false);
  const [otherOpen, setOtherOpen] = useState(false);
  const [detail, setDetail] = useState<string | null>(null);
  const { hereName, others } = useThisAndOthers();
  const [lit, setLit] = useState<string | null>(null);

  useEffect(() => {
    if (!view || !focusApp) return;
    document.querySelector<HTMLElement>(`[data-connector-app="${focusApp}"]`)?.scrollIntoView?.({ block: "center" });
    setLit(focusApp);
    const timer = setTimeout(() => setLit(null), 2000);
    return () => clearTimeout(timer);
  }, [Boolean(view), focusApp]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!view || c.apps.rows === null) return <div className="mb-8 h-48" aria-busy="true" />;
  const open = (agent: string) => void navigate({ to: "/settings/connections/$app", params: { app: agent } });
  const moreOpen = Boolean(c.input.moreOpen || (c.input.query ?? "").trim());
  const detailRow = detail ? ([...view.providers, ...view.moreProviders].find((p) => p.id === detail) ?? null) : null;
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
            {others.length > 0
              ? t("settings.connections.page.on_here_others", { name: hereName, others: others.join(", ") })
              : t("settings.connections.page.on_here", { name: hereName })}
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
            <GroupedRow className="min-h-9 bg-muted/40 py-2 text-xs text-muted-foreground">{t("settings.connections.page.not_installed")}</GroupedRow>
            {view!.more.map((row) => {
              const url = getItUrl(row.agent);
              return (
                <GroupedRow key={row.agent} data-connected-missing={row.agent} className="text-muted-foreground">
                  <HarnessMark agent={row.agent} name={row.name} />
                  <span className="min-w-0 flex-1 text-[13px]">{row.name}</span>
                  {url ? (
                    <button
                      type="button"
                      className="text-xs hover:text-foreground hover:underline"
                      onClick={() => void data?.call("session.openExternal", { url }).catch(() => {})}
                    >
                      {t("settings.connections.page.get_it")}
                    </button>
                  ) : null}
                </GroupedRow>
              );
            })}
          </>
        ) : null}
        <GroupedAction data-connected-show-all="" aria-expanded={all} onClick={() => setAll(!all)}>
          {all ? t("settings.connections.page.show_fewer") : t("settings.connections.page.show_all")}{" "}
          {all ? <ChevronUpIcon className="size-3.5" /> : <ChevronDownIcon className="size-3.5" />}
        </GroupedAction>
      </SettingsGroup>
      <AgentMenuGroup c={c} />
      {all ? (
        <SettingsGroup title={t("settings.connections.page.background_use")}>
          <Suspense fallback={<div className="h-48" />}>
            <DriverSettingsCard />
          </Suspense>
        </SettingsGroup>
      ) : null}

      <SettingsGroup title={t("settings.connections.page.other_computer")} data-connectors-other-machine-group="">
        <GroupedRow>
          <div className="min-w-0 flex-1">
            <div className="text-[13px]">{t("settings.connections.page.other_agent")}</div>
            <div className="text-xs text-muted-foreground">{t("settings.connections.page.other_help", { name: hereName })}</div>
          </div>
          <Button size="sm" variant="outline" aria-expanded={otherOpen} onClick={() => setOtherOpen(!otherOpen)} data-connectors-other-toggle="">
            {otherOpen ? t("settings.connections.page.hide") : t("settings.connections.page.show_how")}
          </Button>
        </GroupedRow>
        {otherOpen ? <OtherMachineBlock /> : null}
        {keys.report && typeof keys.report.copySignIn === "boolean" ? (
          <SettingsRow
            label={t("settings.connections.page.copy_sign_in")}
            description={t("settings.connections.page.copy_sign_in_help", { name: hereName })}
            htmlFor="connectors-copy-sign-in"
            control={
              <Switch
                id="connectors-copy-sign-in"
                data-connectors-copy-sign-in=""
                checked={keys.report.copySignIn}
                onCheckedChange={(on) => void keys.setCopySignIn(on).catch(toastError(t("settings.connections.page.copy_sign_in_failed")))}
              />
            }
          />
        ) : null}
      </SettingsGroup>

      <SettingsGroup title={view.providersTitle} footer={view.providersFooter} data-connectors-providers="">
        {view.providers.map((p) => (
          <ProviderRow key={p.id} row={p} c={c} onOpen={() => setDetail(p.id)} />
        ))}
        {moreOpen ? (
          <>
            <GroupedRow className="gap-2" data-connectors-more="">
              <Input
                type="search"
                data-connectors-search=""
                aria-label={view.moreSearchLabel}
                placeholder={view.moreSearchLabel}
                value={c.input.query ?? ""}
                autoComplete="off"
                spellCheck={false}
                onChange={(e) => c.searchMore(e.currentTarget.value)}
                className="h-8 flex-1"
              />
            </GroupedRow>
            {view.moreProviders.map((p) => (
              <ProviderRow key={p.id} row={p} c={c} onOpen={() => setDetail(p.id)} inMore />
            ))}
            {view.moreEmpty ? (
              <GroupedRow className="text-muted-foreground" data-connectors-more-empty="">
                {view.moreEmpty}
              </GroupedRow>
            ) : null}
          </>
        ) : null}
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
              {t("settings.connections.page.add_key")}
            </Button>
          }
        />
        <GroupedAction data-connectors-more-toggle="" aria-expanded={moreOpen} onClick={() => c.searchMore("", !moreOpen)}>
          {moreOpen ? t("settings.connections.page.show_fewer") : t("settings.connections.page.show_all_count", { count: String(view.moreCount) })}{" "}
          {moreOpen ? <ChevronUpIcon className="size-3.5" /> : <ChevronDownIcon className="size-3.5" />}
        </GroupedAction>
      </SettingsGroup>

      <GithubGroup onPaste={() => keys.openSheet("other", "GITHUB_TOKEN")} saved={keys.input.keys.some((k) => k.env === "GITHUB_TOKEN")} />

      {detailRow ? (
        <ProviderDetail
          row={detailRow}
          addedLabel={view.addedLabel}
          c={c}
          keys={keys}
          onClose={() => setDetail(null)}
          onOpenApp={(agent) => (setDetail(null), open(agent))}
        />
      ) : null}

      {keys.sheet ? <KeySheet key={`${keys.sheet.provider}:${keys.sheet.env ?? ""}`} keys={keys} provider={keys.sheet.provider} env={keys.sheet.env} /> : null}
      <ConfirmDialog
        open={confirm !== null}
        onOpenChange={(o) => !o && keys.askRemove(null)}
        title={confirm?.title ?? ""}
        description={confirm?.message ?? ""}
        confirmLabel={confirm?.confirmLabel ?? t("settings.connections.remove")}
        cancelLabel={confirm?.cancelLabel}
        destructive
        onConfirm={() => {
          const env = keys.removing;
          if (env) void keys.remove(env).catch(toastError(t("settings.connections.keys.remove_failed")));
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
        <Button
          size="sm"
          variant="outline"
          disabled={!row.button.enabled}
          data-connector-connect={row.button.id}
          onClick={(e) => (stop(e), onConnect())}
          onKeyDown={stop}
        >
          {row.state === "connecting" ? <Loader2Icon className="animate-spin" /> : null}
          {row.button.label}
        </Button>
      ) : null}
      <ChevronRightIcon aria-hidden className="size-4 shrink-0 text-muted-foreground/70" data-connector-chevron="" />
    </div>
  );
}

/** A provider: the whole row opens its sheet (hover, chevron); Connect works on its own. */
function ProviderRow({ row, c, onOpen, inMore }: { row: ConnectorProviderRow; c: Connectors; onOpen: () => void; inMore?: boolean }) {
  const t = useT();
  const stop = (e: MouseEvent | KeyboardEvent) => e.stopPropagation();
  const connected = row.state === "connected";
  const problem = row.state === "failed" || row.status === "Needs reconnect";
  // Connected rows say what is saved; the others say why not (Not connected, Needs reconnect, what failed).
  const line = connected && row.credential ? row.credential : row.status;
  return (
    <div
      role="link"
      tabIndex={0}
      aria-label={t("settings.connections.github.row_aria", { title: row.label })}
      title={row.worksWith.length ? `${row.worksWithLabel} ${row.worksWith.map((w) => w.name).join(", ")}` : undefined}
      data-connector-provider={row.id}
      data-state={row.state}
      data-more-list={inMore ? "" : undefined}
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
      )}
    >
      <ProviderMark id={row.id} name={row.label} />
      <div className="min-w-0 flex-1">
        <div className="truncate">{row.label}</div>
        <p
          className={cn("mt-px truncate text-xs", problem ? "text-destructive" : "text-muted-foreground")}
          data-connector-credential=""
          data-connector-status=""
        >
          {line}
        </p>
      </div>
      {connected ? (
        <span className="flex items-center gap-1 text-[13px] text-muted-foreground" data-connector-connected="">
          <CheckIcon className="size-3.5 text-success" /> {row.status}
        </span>
      ) : (
        <Button
          size="sm"
          variant="outline"
          disabled={!row.button.enabled}
          data-connector-provider-action={row.button.id}
          onClick={(e) => (stop(e), c.openProvider(row.id))}
          onKeyDown={stop}
        >
          {row.state === "connecting" ? <Loader2Icon className="animate-spin" /> : null}
          {row.button.label}
        </Button>
      )}
      <ChevronRightIcon aria-hidden className="size-4 shrink-0 text-muted-foreground/70" data-connector-chevron="" />
    </div>
  );
}

/**
 * A provider's sheet: what is saved, when it was added, the apps it works with
 * (each opens the app's page) and Disconnect, or Connect when nothing is saved.
 */
/** Sign-ins are one fixed credential of an app; only API keys can be several. */
const SIGN_IN_ENVS = new Set(["CLAUDE_CODE_OAUTH_TOKEN", "CODEX_AUTH_JSON"]);

function ProviderDetail({
  row,
  addedLabel,
  c,
  keys,
  onClose,
  onOpenApp,
}: {
  row: ConnectorProviderRow;
  addedLabel: string;
  c: Connectors;
  keys: AgentKeysHook;
  onClose: () => void;
  onOpenApp: (agent: string) => void;
}) {
  const t = useT();
  const connected = row.state === "connected";
  const [adding, setAdding] = useState(false);
  // The API keys saved for it: the first, then those added beside it (each named, or by its last four).
  const saved = [...keys.input.keys, ...(keys.input.extras ?? [])].filter((k) => k.provider === row.id && !SIGN_IN_ENVS.has(k.env));
  const several = saved.length > 1;
  const removeKey = (k: (typeof saved)[number]) => {
    const others = saved.some((x) => x.env === k.env && x !== k);
    void (k.slot ? keys.remove(k.env, { slot: k.slot }) : keys.remove(k.env, others ? { keepOthers: true } : {}))
      .then(() => c.reload())
      .catch(toastError(t("settings.connections.keys.remove_failed")));
  };
  if (adding) return <AddKeyDialog row={row} keys={keys} onDone={() => (setAdding(false), void c.reload())} onCancel={() => setAdding(false)} />;
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogPopup data-provider-detail={row.id} className="top-[12vh] w-[min(480px,calc(100vw-2rem))]">
        <div className="px-6 pt-5 pb-5">
          <div className="flex items-center gap-3">
            <ProviderMark id={row.id} name={row.label} large />
            <div className="min-w-0 flex-1">
              <DialogTitle className="truncate text-[15px] font-semibold">{row.label}</DialogTitle>
              <DialogDescription className="truncate text-[13px] text-muted-foreground">
                {connected && row.credential ? row.credential : row.status}
              </DialogDescription>
            </div>
          </div>
          {row.addedMs !== null ? (
            <FactRow
              className="mt-3"
              label={t("settings.connections.detail.added")}
              value={addedText(addedLabel, row.addedMs).replace(/^Added /, "")}
              data-connector-added=""
            />
          ) : null}
          {several ? (
            <div className="mt-3" data-connector-keys="">
              <div className="px-1 text-xs text-muted-foreground">{t("settings.connections.detail.keys")}</div>
              {saved.map((k) => (
                <div key={`${k.env}:${k.slot ?? ""}`} className="flex items-center gap-2 px-1 py-1 text-[13px]" data-connector-key={k.slot ?? "first"}>
                  <span className="min-w-0 flex-1 truncate">
                    {k.label?.trim() || (k.last4 ? t("settings.connections.detail.key_ending", { last4: k.last4 }) : t("settings.connections.detail.key"))}
                  </span>
                  <Button size="sm" variant="ghost" onClick={() => removeKey(k)} data-connector-key-remove="">
                    {t("settings.connections.remove")}
                  </Button>
                </div>
              ))}
            </div>
          ) : null}
          {row.worksWith.length ? (
            <div className="mt-3" data-connector-works-with="">
              <div className="px-1 text-xs text-muted-foreground">{row.worksWithLabel}</div>
              <div className="mt-1 flex flex-col">
                {row.worksWith.map((chip) => (
                  <button
                    key={chip.agent}
                    type="button"
                    data-connector-chip={chip.agent}
                    onClick={() => onOpenApp(chip.agent)}
                    className="-mx-1 flex w-[calc(100%+0.5rem)] cursor-default items-center gap-2.5 rounded-lg px-2 py-1.5 text-left text-[13px] outline-none hover:bg-accent focus-visible:bg-accent"
                  >
                    <HarnessMark agent={chip.agent} name={chip.name} />
                    <span className="min-w-0 flex-1 truncate">{chip.name}</span>
                    <ChevronRightIcon aria-hidden className="size-3.5 shrink-0 text-muted-foreground/60" />
                  </button>
                ))}
              </div>
            </div>
          ) : null}
          <div className="mt-4 flex justify-end gap-2">
            {connected && saved.length > 0 ? (
              <Button variant="ghost" className="mr-auto" onClick={() => setAdding(true)} data-connector-add-key="">
                {t("settings.connections.detail.add_another")}
              </Button>
            ) : null}
            <Button variant="outline" onClick={onClose}>
              {t("settings.connections.close")}
            </Button>
            <Button
              variant={connected ? "outline" : "default"}
              disabled={!row.button.enabled}
              data-connector-provider-action={row.button.id}
              onClick={() => {
                onClose();
                if (row.button.id === "sign-out") void c.signOut(row.id);
                else c.openProvider(row.id);
              }}
            >
              {row.button.label}
            </Button>
          </div>
        </div>
      </DialogPopup>
    </Dialog>
  );
}

/** "Add another key": a second API key for a provider, named so it can be told apart ("Work key"). It replaces nothing. */
function AddKeyDialog({ row, keys, onDone, onCancel }: { row: ConnectorProviderRow; keys: AgentKeysHook; onDone: () => void; onCancel: () => void }) {
  const t = useT();
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await keys.save(row.id, value.trim(), null, {}, { add: true, ...(name.trim() ? { label: name.trim() } : {}) });
      onDone();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  };
  return (
    <Dialog open onOpenChange={(o) => !o && !busy && onCancel()}>
      <DialogPopup data-connector-add-key-sheet={row.id} className="top-[12vh] w-[min(440px,calc(100vw-2rem))]">
        <div className="px-6 pt-5 pb-5">
          <DialogTitle className="text-[15px] font-semibold">{t("settings.connections.add_key.title", { label: row.label })}</DialogTitle>
          <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{t("settings.connections.add_key.help")}</DialogDescription>
          <label className="mt-3 block">
            <span className="mb-1 block text-xs font-medium">{t("settings.connections.name_optional")}</span>
            <Input
              value={name}
              maxLength={40}
              placeholder={t("settings.connections.add_key.placeholder")}
              autoComplete="off"
              onChange={(e) => setName(e.currentTarget.value)}
              data-add-key-name=""
            />
          </label>
          <label className="mt-3 block">
            <span className="mb-1 block text-xs font-medium">{t("settings.connections.add_key.key")}</span>
            <Input
              type="password"
              value={value}
              autoComplete="off"
              spellCheck={false}
              className="font-mono"
              onChange={(e) => setValue(e.currentTarget.value)}
              data-add-key-value=""
            />
          </label>
          {error ? (
            <p className="mt-2 text-[13px] text-destructive" data-add-key-error="">
              {error}
            </p>
          ) : null}
          <div className="mt-4 flex justify-end gap-2">
            <Button variant="outline" disabled={busy} onClick={onCancel}>
              {t("settings.connections.cancel")}
            </Button>
            <Button disabled={busy || !value.trim()} onClick={() => void save()} data-add-key-save="">
              {busy ? <Loader2Icon className="animate-spin" /> : null}
              {t("settings.connections.save")}
            </Button>
          </div>
        </div>
      </DialogPopup>
    </Dialog>
  );
}
