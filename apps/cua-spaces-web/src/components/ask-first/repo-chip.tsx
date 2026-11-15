// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { GitBranchIcon, XIcon } from "lucide-react";
import { useRef, useState } from "react";

import { Popover } from "@/components/ui/popover";
import { chooseRepo, clearRepo, parseRepo, useRepos, type AskRepo } from "@/lib/ask-first-repo";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";

/**
 * The optional repo of the task: an empty "Add a repo" that opens a field
 * for a link or `owner/name` and the repos used lately, then the repo as a
 * chip with a way to take it off.
 */
export function RepoChip({ repo, onRepo }: { repo: AskRepo | null; onRepo?: (r: AskRepo | null) => void }) {
  const t = useT();
  const { recent } = useRepos();
  const [open, setOpen] = useState(false);
  const [text, setText] = useState("");
  const [bad, setBad] = useState(false);
  const field = useRef<HTMLInputElement>(null);
  const take = (r: AskRepo) => {
    chooseRepo(r);
    onRepo?.(r);
    setOpen(false);
    setText("");
    setBad(false);
  };
  const submit = () => {
    const r = parseRepo(text);
    if (!r) {
      setBad(true);
      return;
    }
    take(r);
  };
  if (repo) {
    return (
      <span data-ask-repo={repo.label} className="flex h-9 items-center gap-1.5 rounded-lg bg-foreground/[0.06] pr-1 pl-2.5 text-[13px]">
        <GitBranchIcon className="size-3.5 text-muted-foreground" />
        <span className="max-w-56 truncate font-medium">{repo.label}</span>
        <button
          type="button"
          aria-label={t("home.repo.remove", { repo: repo.label })}
          data-ask-repo-remove=""
          onClick={() => {
            clearRepo();
            onRepo?.(null);
          }}
          className="flex size-6 items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-foreground/10 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
        >
          <XIcon className="size-3.5" />
        </button>
      </span>
    );
  }
  return (
    <Popover
      open={open}
      onOpenChange={setOpen}
      width={340}
      initialFocus={field}
      trigger={
        <button
          type="button"
          data-picker="repo"
          className="flex h-9 items-center gap-2 rounded-lg px-2.5 text-[13px] text-muted-foreground outline-none hover:bg-foreground/5 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 data-popup-open:bg-foreground/[0.07]"
        >
          <GitBranchIcon className="size-3.5" />
          {t("home.repo.add")}
        </button>
      }
    >
      <input
        ref={field}
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setBad(false);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            submit();
          }
        }}
        placeholder={t("home.repo.placeholder")}
        aria-label={t("home.repo.field_label")}
        aria-invalid={bad}
        className={cn(
          "w-full rounded-lg bg-foreground/[0.05] px-2.5 py-2 text-[13px] outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring/60",
          bad && "ring-2 ring-destructive/50",
        )}
      />
      {bad ? <p className="px-2.5 pt-1.5 text-[12px] text-destructive">{t("home.repo.invalid")}</p> : null}
      {recent.length ? (
        <div className="mt-1 pt-1">
          <p className="px-2.5 pt-1 pb-0.5 text-[12px] font-medium text-muted-foreground">{t("home.repo.recent")}</p>
          {recent.map((r) => (
            <button
              type="button"
              key={r.url}
              data-repo-recent={r.label}
              onClick={() => take(r)}
              className="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[13px] outline-none hover:bg-foreground/[0.06] focus-visible:ring-2 focus-visible:ring-ring/60"
            >
              <GitBranchIcon className="size-3.5 text-muted-foreground" />
              <span className="truncate">{r.label}</span>
            </button>
          ))}
        </div>
      ) : null}
    </Popover>
  );
}
