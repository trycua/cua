// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ChevronRightIcon } from "lucide-react";

import type { CiAction, CiHelpTopic } from "@/bridge";
import { GroupedSection } from "@/components/ui/grouped";
import { cn } from "@/lib/utils";

/** The common problems, one line each; one opens at a time. */
export function Help({ title, topics, onAction }: { title: string; topics: CiHelpTopic[]; onAction(a: CiAction): void }) {
  return (
    <GroupedSection title={title} data-ci-help="">
      {topics.map((t) => (
        <div key={t.id} data-ci-help-topic={t.id} data-open={t.open ? "true" : undefined}>
          <button
            type="button"
            aria-expanded={t.open}
            onClick={() => onAction({ type: "help", id: t.id })}
            className="flex min-h-11 w-full cursor-default items-center gap-2.5 px-4 py-2.5 text-left text-[13px] outline-none hover:bg-accent/50 focus-visible:bg-accent/50"
          >
            <ChevronRightIcon className={cn("size-3.5 shrink-0 text-muted-foreground transition-transform", t.open && "rotate-90")} aria-hidden />
            {t.title}
          </button>
          {t.body ? <p className="pr-4 pb-3.5 pl-[2.6rem] text-[13px] text-muted-foreground">{t.body}</p> : null}
        </div>
      ))}
    </GroupedSection>
  );
}
