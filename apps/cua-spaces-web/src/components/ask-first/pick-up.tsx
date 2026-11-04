// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState } from "react";

import { folderLabel, isUnsupported, useBridge, type RecentThread } from "@/bridge";
import { Button } from "@/components/ui/button";
import { GroupedRow, GroupedSection } from "@/components/ui/grouped";
import { addImportedTask, type AskTask } from "@/lib/ask-first";
import { cn } from "@/lib/utils";

import { AppMark } from "./ask-box";

/** How many chats the Ready screen offers. */
export const PICK_UP_LIMIT = 4;

/**
 * The recent Claude Code, Codex and Hermes chats on this computer
 * (`agents.recentThreads`), newest first. Null until the host answered;
 * empty when there are none or the host can't list them.
 */
export function useRecentThreads(limit = PICK_UP_LIMIT): RecentThread[] | null {
  const { data } = useBridge();
  const [threads, setThreads] = useState<RecentThread[] | null>(null);
  useEffect(() => {
    let live = true;
    if (!data) return;
    data
      .call("agents.recentThreads", { limit })
      .then((rows) => live && setThreads(rows))
      .catch((e: unknown) => {
        // A host or daemon without the list, or a store it couldn't read: the section stays hidden.
        if (live) setThreads([]);
        if (!isUnsupported(e)) console.warn("agents.recentThreads failed", e);
      });
    return () => {
      live = false;
    };
  }, [data, limit]);
  return threads;
}

/** "25 min ago", "3 h ago", "2 days ago". */
export function agoLabel(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.round(h / 24);
  return d === 1 ? "yesterday" : `${d} days ago`;
}

/**
 * "Pick up where you left off" on the Ready screen (CUA-3319, behind the new
 * setup): 2 to 4 of the person's recent chats with their agents on this Mac,
 * each with **Continue in Cua**, which opens it in a new Home task with what
 * was said so far. Nothing runs until they send the next message. Hidden
 * when there are none. Only each chat's title, folder and time are read
 * here; its messages are read when it is picked.
 */
export function PickUpWhereYouLeftOff({ onContinue, className }: { onContinue: (task: AskTask) => void; className?: string }) {
  const threads = useRecentThreads();
  if (!threads?.length) return null;
  return (
    <div className={cn("flex flex-col gap-2", className)} data-pick-up="">
      <h2 id="pick-up-heading" className="text-[15px] font-semibold">
        Pick up where you left off
      </h2>
      <GroupedSection aria-labelledby="pick-up-heading" role="list" footer="Your chats stay on this Mac until you pick one.">
        {threads.map((t) => (
          <ThreadRow key={`${t.agent}:${t.id}`} thread={t} onContinue={() => onContinue(addImportedTask(t))} />
        ))}
      </GroupedSection>
    </div>
  );
}

function ThreadRow({ thread, onContinue }: { thread: RecentThread; onContinue: () => void }) {
  const folder = folderLabel(thread.cwd);
  return (
    <GroupedRow role="listitem" className="flex items-center gap-3 px-4 py-3" data-pick-up-row={thread.agent}>
      <AppMark agent={thread.agent} size={32} />
      <div className="min-w-0 flex-1">
        <div className="truncate text-[14px] font-medium">{thread.title}</div>
        <div className="truncate text-[12px] text-muted-foreground">{folder ? `${folder}, ${agoLabel(thread.lastActiveMs)}` : agoLabel(thread.lastActiveMs)}</div>
      </div>
      <Button variant="outline" className="h-9 shrink-0 px-4 text-[13px]" onClick={onContinue} data-pick-up-continue="">
        Continue in Cua
      </Button>
    </GroupedRow>
  );
}
