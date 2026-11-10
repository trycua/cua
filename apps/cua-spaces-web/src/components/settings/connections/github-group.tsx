// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronRightIcon, Loader2Icon, TriangleAlertIcon } from "lucide-react";
import { useState } from "react";

import { useGithub, type GithubDetail, type GithubHook, type GithubRow } from "@/bridge";
import { ProviderMark } from "@/components/settings/connections/provider-mark";
import { SettingsGroup } from "@/components/settings-group";
import { Button } from "@/components/ui/button";
import { Dialog, DialogDescription, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { GroupedAction, GroupedRow } from "@/components/ui/grouped";
import { Switch } from "@/components/ui/switch";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";

/**
 * Settings, Connections, GitHub: the sign-in Cua uses when an agent asks to
 * push a branch and open a pull request. One row per host with its state in
 * plain words; the whole row opens the page behind it (the switch, the
 * account picker when gh has several accounts, how Cua signs in), and Rescan
 * looks again. Every word is the core's (`githubConnection.view`); a pasted
 * token goes in through the existing key sheet (`onPaste`).
 */
export function GithubGroup({ onPaste, saved }: { onPaste: () => void; saved: boolean }) {
  const g = useGithub(true, saved);
  const [open, setOpen] = useState<string | null>(null);
  const { view } = g;
  if (!view) return null;
  const detail = view.details.find((d) => d.host === open) ?? null;
  return (
    <>
      <SettingsGroup title={view.title} footer={view.footer} data-connectors-github="">
        {view.notice ? (
          <GroupedRow className="text-muted-foreground" data-github-notice="">
            {view.notice}
          </GroupedRow>
        ) : null}
        {view.rows.map((row) => (
          <HostRow key={row.host} row={row} onOpen={() => setOpen(row.host)} />
        ))}
        <GroupedAction data-github-rescan="" disabled={g.rescanning} onClick={g.rescan}>
          {g.rescanning ? <Loader2Icon className="size-3.5 animate-spin" /> : null}
          {view.rescanLabel}
        </GroupedAction>
      </SettingsGroup>
      {detail ? (
        <HostSheet
          d={detail}
          g={g}
          onClose={() => setOpen(null)}
          onPaste={() => {
            setOpen(null);
            onPaste();
          }}
        />
      ) : null}
    </>
  );
}

/** A host: the whole row opens its page (hover, chevron). */
function HostRow({ row, onOpen }: { row: GithubRow; onOpen: () => void }) {
  const t = useT();
  return (
    <div
      role="link"
      tabIndex={0}
      aria-label={t("settings.connections.github.row_aria", { title: row.title })}
      data-github-host={row.host}
      data-tone={row.tone}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen();
        }
      }}
      className="flex min-h-12 cursor-default items-center gap-3 px-4 py-2.5 text-[13px] outline-none transition-colors hover:bg-accent/50 focus-visible:bg-accent/50"
    >
      <ProviderMark id="github-source" name="GitHub" />
      <div className="min-w-0 flex-1">
        <div className="truncate">{row.title}</div>
        <p className={cn("mt-px truncate text-xs", row.tone === "attention" ? "text-destructive" : "text-muted-foreground")} data-github-line="">
          {row.line}
        </p>
      </div>
      {row.tone === "ok" ? <CheckIcon aria-hidden className="size-3.5 text-success" data-github-ok="" /> : null}
      {row.warning ? (
        <TriangleAlertIcon aria-label={t("settings.connections.github.broader")} className="size-4 text-warning" data-github-warning-mark="" />
      ) : null}
      <ChevronRightIcon aria-hidden className="size-4 shrink-0 text-muted-foreground/70" />
    </div>
  );
}

/** One host's page: the switch, the account picker, how Cua signs in, what to do when nothing is. */
function HostSheet({ d, g, onClose, onPaste }: { d: GithubDetail; g: GithubHook; onClose: () => void; onPaste: () => void }) {
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogPopup data-github-detail={d.host} className="top-[12vh] w-[min(480px,calc(100vw-2rem))]">
        <div className="px-6 pt-5 pb-5">
          <div className="flex items-center gap-3">
            <ProviderMark id="github-source" name="GitHub" large />
            <div className="min-w-0 flex-1">
              <DialogTitle className="truncate text-[15px] font-semibold">{d.title}</DialogTitle>
              <DialogDescription className="text-[13px] text-muted-foreground" data-github-detail-line="">
                {d.line}
              </DialogDescription>
            </div>
          </div>
          {d.warning ? (
            <div className="mt-3 flex items-start gap-2.5 rounded-lg bg-warning/10 px-3 py-2 text-[13px]" data-github-warning="">
              <TriangleAlertIcon className="mt-0.5 size-4 shrink-0 text-warning" />
              <span className="min-w-0 flex-1">{d.warning}</span>
            </div>
          ) : null}
          <label className="mt-4 flex items-start gap-3 text-[13px]" htmlFor="github-enabled">
            <div className="min-w-0 flex-1">
              <div>{d.switchLabel}</div>
              <div className="mt-px text-xs text-muted-foreground">{d.switchHelp}</div>
            </div>
            <Switch id="github-enabled" data-github-switch="" checked={d.enabled} onCheckedChange={(on) => g.setEnabled(d.host, on)} />
          </label>
          {d.accountsLabel ? (
            <div className="mt-4" data-github-accounts="">
              <div className="px-1 text-xs font-medium">{d.accountsLabel}</div>
              <p className="px-1 text-xs text-muted-foreground">{d.accountsHelp}</p>
              <div className="mt-1 flex flex-col">
                {d.accounts.map((a) => (
                  <button
                    key={a.login}
                    type="button"
                    role="radio"
                    aria-checked={a.selected}
                    data-github-account={a.login}
                    onClick={() => g.choose(d.host, a.login)}
                    className="-mx-1 flex w-[calc(100%+0.5rem)] cursor-default items-center gap-2.5 rounded-lg px-2 py-1.5 text-left text-[13px] outline-none hover:bg-accent focus-visible:bg-accent"
                  >
                    <span className="flex size-4 items-center justify-center">{a.selected ? <CheckIcon className="size-3.5" /> : null}</span>
                    <span className="min-w-0 flex-1 truncate">
                      {a.login}
                      {a.tag ? <span className="ml-2 text-xs text-muted-foreground">{a.tag}</span> : null}
                      {a.note ? <span className="ml-2 text-xs text-destructive">{a.note}</span> : null}
                    </span>
                  </button>
                ))}
              </div>
              {d.autoLabel ? (
                <Button size="sm" variant="ghost" className="mt-1" data-github-auto="" onClick={() => g.choose(d.host, null)}>
                  {d.autoLabel}
                </Button>
              ) : null}
            </div>
          ) : null}
          <div className="mt-4" data-github-sources="">
            <div className="px-1 text-xs font-medium">{d.sourcesTitle}</div>
            <div className="mt-1 flex flex-col">
              {d.sources.map((s) => (
                <div key={s.id} className="flex items-center gap-2 px-1 py-1 text-[13px]" data-github-source={s.id}>
                  <span className={cn("min-w-0 flex-1", !s.tag && "text-muted-foreground")}>{s.line}</span>
                  {s.tag ? <span className="text-xs text-muted-foreground">{s.tag}</span> : null}
                </div>
              ))}
            </div>
          </div>
          {d.help ? (
            <p className="mt-3 text-[13px] text-muted-foreground" data-github-help="">
              {d.help}
            </p>
          ) : null}
          <div className="mt-4 flex justify-end gap-2">
            <Button variant="ghost" className="mr-auto" disabled={g.rescanning} data-github-detail-rescan="" onClick={g.rescan}>
              {g.rescanning ? <Loader2Icon className="animate-spin" /> : null}
              {d.rescanLabel}
            </Button>
            {d.pasteLabel ? (
              <Button variant="outline" data-github-paste="" onClick={onPaste}>
                {d.pasteLabel}
              </Button>
            ) : null}
            <Button variant="outline" onClick={onClose}>
              {d.closeLabel}
            </Button>
          </div>
        </div>
      </DialogPopup>
    </Dialog>
  );
}
