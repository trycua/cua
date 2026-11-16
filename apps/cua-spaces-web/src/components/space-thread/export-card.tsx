// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ChevronRightIcon, GitBranchIcon } from "lucide-react";
import { useState } from "react";

import type { CardFileRow, ExportCardView, Square } from "@/bridge";
import { cn } from "@/lib/utils";

/**
 * What the host asks about before something leaves a Space, kept small: the
 * headline, `branch into base`, one diff line (the file count, +N in green,
 * -N in red) and the account. The files with their five-square bars, the
 * commits and the pull request that will open are behind one `Show changes`.
 * Every word and number is the app core's (`ExportCardView`); this only
 * places them and colours the diff with the theme's tokens (`--diff-add`,
 * `--diff-del`). The buttons are the approval's own.
 */

const SQUARE: Record<Square, string> = { add: "bg-diff-add-bar", del: "bg-diff-del-bar", none: "bg-diff-none-bar" };

/** `+12 -3`, each in its colour; nothing for a side with no lines. */
function Counts({ additions, deletions, className }: { additions: string; deletions: string; className?: string }) {
  if (!additions && !deletions) return null;
  return (
    <span className={cn("inline-flex shrink-0 items-baseline gap-1.5 font-mono tabular-nums", className)}>
      {additions ? (
        <span data-diff="add" className="text-diff-add">
          {additions}
        </span>
      ) : null}
      {deletions ? (
        <span data-diff="del" className="text-diff-del">
          {deletions}
        </span>
      ) : null}
    </span>
  );
}

/** Five squares in proportion (decorative: the counts say the same in words). */
function Bar({ bar }: { bar: CardFileRow["bar"] }) {
  return (
    <span aria-hidden data-bar={bar.join(",")} className="inline-flex shrink-0 gap-px">
      {bar.map((s, i) => (
        <span key={i} className={cn("size-[7px]", SQUARE[s])} />
      ))}
    </span>
  );
}

function FileLine({ f }: { f: CardFileRow }) {
  return (
    <li data-file={f.name} data-status={f.status} className="flex min-h-6 items-center gap-2 text-[12px] leading-5">
      <span className="min-w-0 flex-1 truncate font-mono" title={`${f.dir}${f.name}${f.note ? `, ${f.note}` : ""}`}>
        <span className="text-muted-foreground">{f.dir}</span>
        <span>{f.name}</span>
        {f.note ? <span className="ml-2 font-sans text-[11px] text-muted-foreground">{f.note}</span> : null}
      </span>
      <Counts additions={f.additions} deletions={f.deletions} className="text-[11.5px]" />
      <Bar bar={f.bar} />
    </li>
  );
}

export function ExportCard({ view }: { view: ExportCardView }) {
  const [open, setOpen] = useState(false);
  return (
    <div data-export-card="" className="mt-0.5 space-y-1 text-[12.5px]">
      <p data-card-line="headline" className="text-[13px]">
        {view.headline}
      </p>
      <p data-card-line="branch" data-branch-line="" className="flex items-center gap-1.5 text-muted-foreground">
        <GitBranchIcon className="size-3.5 shrink-0" aria-hidden />
        <span className="min-w-0 truncate font-mono text-[12px]" title={view.branchLine}>
          <span className="text-foreground">{view.branch}</span>
          {view.branchLine.slice(view.branch.length)}
        </span>
      </p>
      <p data-card-line="diff" data-totals="" className="flex items-baseline gap-2 text-muted-foreground">
        <span>{view.totals.files}</span>
        <Counts additions={view.totals.additions} deletions={view.totals.deletions} />
      </p>
      {view.account ? (
        <p data-card-line="account" data-account="" className="text-[11.5px] text-muted-foreground">
          {view.account}
        </p>
      ) : null}
      {view.expandable ? (
        <button
          type="button"
          data-card-line="expander"
          data-card-expander=""
          aria-expanded={open}
          onClick={() => setOpen((o) => !o)}
          className="-ml-1 flex cursor-default items-center gap-1 rounded-lg px-1 py-0.5 text-[11.5px] text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
        >
          <ChevronRightIcon className={cn("size-3 transition-transform duration-150", open && "rotate-90")} aria-hidden />
          {open ? view.hideLabel : view.showLabel}
        </button>
      ) : null}
      {open ? (
        <div data-card-changes="" className="space-y-3 pt-1">
          {view.files.length ? (
            <section aria-label="Files changed">
              <ul>
                {view.files.map((f) => (
                  <FileLine key={`${f.dir}${f.name}`} f={f} />
                ))}
              </ul>
              {view.moreFiles ? <p className="text-[11.5px] text-muted-foreground">{view.moreFiles}</p> : null}
            </section>
          ) : null}
          {view.commits.length ? (
            <section aria-label="Commits">
              <p className="text-[11.5px] text-muted-foreground">{view.commitsHeading}</p>
              <ul className="mt-0.5 space-y-0.5">
                {view.commits.map((c) => (
                  <li key={c.sha} data-commit={c.sha} className="flex items-baseline gap-2 leading-5">
                    <code className="shrink-0 font-mono text-[11.5px] text-muted-foreground">{c.sha}</code>
                    <span className="min-w-0 flex-1 truncate" title={c.subject}>
                      {c.subject}
                    </span>
                    <span className="shrink-0 text-[11.5px] text-muted-foreground">{c.author}</span>
                  </li>
                ))}
              </ul>
              {view.moreCommits ? <p className="text-[11.5px] text-muted-foreground">{view.moreCommits}</p> : null}
            </section>
          ) : null}
          {view.pr ? (
            <section aria-label="Pull request" data-pr="" className="rounded-lg bg-background/60 px-2.5 py-2">
              <p className="flex items-center gap-2">
                <span className="min-w-0 flex-1 truncate font-medium">{view.pr.title}</span>
                <span
                  data-pr-state={view.pr.state}
                  className={cn(
                    "shrink-0 rounded-lg bg-muted px-1.5 text-[11px] leading-5 font-medium",
                    view.pr.state === "Open" ? "text-diff-add" : "text-muted-foreground",
                  )}
                >
                  {view.pr.state}
                </span>
              </p>
              {view.pr.body.map((l, i) => (
                <p key={i} className="truncate text-[12px] text-muted-foreground" title={l}>
                  {l}
                </p>
              ))}
            </section>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
