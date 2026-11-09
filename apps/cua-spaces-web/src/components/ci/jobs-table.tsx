// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronRightIcon, ExternalLinkIcon, Loader2Icon, XIcon } from "lucide-react";

import type { CiAction, CiJobDetail, CiJobRowView, CiJobsView, CiLog, CiTab } from "@/bridge";
import { Button } from "@/components/ui/button";
import { GroupedSection } from "@/components/ui/grouped";
import { Segmented } from "@/components/ui/segmented";
import { cn } from "@/lib/utils";

/** Columns: job and repository, Wait, Run, Cost, When. */
const GRID = "grid grid-cols-[minmax(0,1fr)_4.5rem_5.5rem_4rem_6.5rem] items-center gap-x-4";

/**
 * The jobs table: tabs All, Running and Failed, and a row per job (its name
 * and repository, how long it waited for a computer, how long it ran, what it
 * cost and when). A row opens to its timeline, cost, computer and cloud, Open
 * on GitHub, and the job's own log. Everything shown is the core's.
 */
export function JobsTable({
  jobs,
  logs,
  onAction,
  onOpen,
  onShowLog,
}: {
  jobs: CiJobsView;
  logs: Record<string, CiLog>;
  onAction(a: CiAction): void;
  onOpen(url: string): void;
  onShowLog(ref: { jobKey: string; pool: string; slot: string }): void;
}) {
  return (
    <section data-ci-jobs="" className="min-w-0">
      <div className="mb-1.5 flex min-h-7 items-center justify-between gap-3 px-1">
        <h2 className="text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">{jobs.title}</h2>
        <Segmented<CiTab>
          aria-label={jobs.title}
          value={jobs.tabs.find((t) => t.selected)?.id ?? "all"}
          options={jobs.tabs.map((t) => ({
            value: t.id,
            label: (
              <span data-ci-tab={t.id}>
                {t.label}
                <span className="ml-1.5 text-muted-foreground tabular-nums">{t.count}</span>
              </span>
            ),
          }))}
          onValueChange={(tab) => onAction({ type: "tab", tab })}
        />
      </div>
      <GroupedSection>
        {jobs.rows.length > 0 ? (
          <div className={cn(GRID, "px-4 py-2 text-xs text-muted-foreground")} data-ci-columns="">
            <span>{jobs.columns[0]}</span>
            <span className="text-right">{jobs.columns[1]}</span>
            <span className="text-right">{jobs.columns[2]}</span>
            <span className="text-right">{jobs.columns[3]}</span>
            <span className="text-right">{jobs.columns[4]}</span>
          </div>
        ) : null}
        {jobs.rows.length > 0 ? (
          jobs.rows.map((r) => <JobRow key={r.key} row={r} log={logs[r.key]} onAction={onAction} onOpen={onOpen} onShowLog={onShowLog} />)
        ) : (
          <p className="px-4 py-8 text-center text-[13px] text-muted-foreground" data-ci-jobs-empty="">
            {jobs.empty}
          </p>
        )}
      </GroupedSection>
    </section>
  );
}

function StateMark({ state }: { state: string }) {
  if (state === "passed") return <CheckIcon className="size-3.5 shrink-0 text-success" aria-hidden />;
  if (state === "failed") return <XIcon className="size-3.5 shrink-0 text-destructive" aria-hidden />;
  return <Loader2Icon className="size-3.5 shrink-0 animate-spin text-muted-foreground motion-reduce:animate-none" aria-hidden />;
}

function JobRow({
  row,
  log,
  onAction,
  onOpen,
  onShowLog,
}: {
  row: CiJobRowView;
  log?: CiLog;
  onAction(a: CiAction): void;
  onOpen(url: string): void;
  onShowLog(ref: { jobKey: string; pool: string; slot: string }): void;
}) {
  return (
    <div data-ci-job={row.key} data-state={row.state} data-expanded={row.expanded ? "true" : undefined}>
      <button
        type="button"
        aria-expanded={row.expanded}
        onClick={() => onAction({ type: "expand", key: row.key })}
        className={cn(GRID, "w-full cursor-default px-4 py-2.5 text-left text-[13px] outline-none hover:bg-accent/50 focus-visible:bg-accent/50")}
      >
        <span className="flex min-w-0 items-center gap-2.5">
          <ChevronRightIcon className={cn("size-3.5 shrink-0 text-muted-foreground transition-transform", row.expanded && "rotate-90")} aria-hidden />
          <StateMark state={row.state} />
          <span className="min-w-0">
            <span className="block truncate">{row.title}</span>
            <span className="block truncate text-xs text-muted-foreground">{row.repo}</span>
          </span>
        </span>
        <span className="text-right tabular-nums text-muted-foreground" data-ci-cell="wait">
          {row.wait}
        </span>
        <span className="text-right tabular-nums text-muted-foreground" data-ci-cell="run">
          {row.run}
        </span>
        <span className="text-right tabular-nums text-muted-foreground" data-ci-cell="cost">
          {row.cost}
        </span>
        <span className="text-right text-muted-foreground" data-ci-cell="when">
          {row.when}
        </span>
      </button>
      {row.detail ? <JobDetail detail={row.detail} log={log} onOpen={onOpen} onShowLog={onShowLog} /> : null}
    </div>
  );
}

function JobDetail({
  detail,
  log,
  onOpen,
  onShowLog,
}: {
  detail: CiJobDetail;
  log?: CiLog;
  onOpen(url: string): void;
  onShowLog(ref: { jobKey: string; pool: string; slot: string }): void;
}) {
  const t = detail.timeline;
  return (
    <div className="pr-4 pb-3.5 pl-[3.1rem]" data-ci-detail="">
      {t ? (
        <div data-ci-timeline="" className="mb-3">
          <div className="flex h-1.5 w-full overflow-hidden rounded-lg bg-muted" role="img" aria-label={`${t.waitLabel}, ${t.runLabel}`}>
            <span className="h-full bg-warning/70" style={{ width: `${t.waitPct}%` }} />
            <span className="h-full bg-brand" style={{ width: `${t.runPct}%` }} />
          </div>
          <div className="mt-1.5 flex flex-wrap gap-x-5 gap-y-0.5 text-xs text-muted-foreground">
            <span className="inline-flex items-center gap-1.5">
              <span className="size-1.5 rounded-full bg-warning/70" aria-hidden />
              {t.waitLabel}
            </span>
            <span className="inline-flex items-center gap-1.5">
              <span className="size-1.5 rounded-full bg-brand" aria-hidden />
              {t.runLabel}
            </span>
          </div>
        </div>
      ) : null}
      <dl className="grid grid-cols-[6.5rem_minmax(0,1fr)] gap-x-4 gap-y-1 text-[13px]">
        {detail.facts.map((f) => (
          <div key={f.label} className="contents" data-ci-fact={f.label}>
            <dt className="text-muted-foreground">{f.label}</dt>
            <dd className="min-w-0 select-text">{f.value}</dd>
          </div>
        ))}
      </dl>
      <div className="mt-3 flex items-center gap-2">
        {detail.openUrl ? (
          <Button size="sm" variant="outline" data-ci-open-github="" onClick={() => onOpen(detail.openUrl!)}>
            <ExternalLinkIcon />
            {detail.openLabel}
          </Button>
        ) : null}
        <Button size="sm" variant="outline" data-ci-show-log="" onClick={() => onShowLog(detail.log)}>
          {detail.logLabel}
        </Button>
      </div>
      {log ? (
        <div className="mt-3 rounded-lg bg-muted px-3 py-2.5" data-ci-log="">
          {log.error ? (
            <p className="text-xs text-destructive">{log.error}</p>
          ) : log.lines === null ? (
            <p className="text-xs text-muted-foreground">Reading the log</p>
          ) : log.lines.length === 0 ? (
            <p className="text-xs text-muted-foreground">Nothing was recorded for this job on this computer.</p>
          ) : (
            <ol className="space-y-0.5 text-xs">
              {log.lines.map((l, i) => (
                <li key={i} className="flex gap-3">
                  <time className="shrink-0 tabular-nums text-muted-foreground" dateTime={new Date(l.at_ms).toISOString()}>
                    {new Date(l.at_ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })}
                  </time>
                  <span className="min-w-0 select-text">{l.text}</span>
                </li>
              ))}
            </ol>
          )}
        </div>
      ) : null}
    </div>
  );
}
