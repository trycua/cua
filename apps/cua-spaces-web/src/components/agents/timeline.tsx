// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { LegendList, type LegendListRef, type MaintainScrollAtEndOptions } from "@legendapp/list/react";
import { ArrowDownIcon, ChevronRightIcon } from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { AgentTimeline, TranscriptItem } from "@/bridge";
import { Markdown } from "@/components/markdown";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/**
 * A run's conversation, after T3 Code's MessagesTimeline: a virtualized
 * LegendList that follows the end while the run writes, lets go when the
 * reader scrolls up (a pill brings them back), and keeps their place as
 * rows above grow. Rows are memoized by item; the transcript replaces only
 * the items an event touched, so while a message streams only its row,
 * and inside it only the last Markdown block, renders again.
 */

const FOLLOW: MaintainScrollAtEndOptions = { animated: false, on: { dataChange: true, itemLayout: true, layout: true, footerLayout: true } };
/** How close to the end counts as at the end (px). */
const END_SLOP = 32;
/** A scroll within this long after the reader's own input is theirs (ms). */
const INTENT_MS = 600;

type Row = TranscriptItem;

export function Timeline({
  timeline,
  className,
  onOpenLink,
  footer,
}: {
  timeline: AgentTimeline;
  className?: string;
  onOpenLink?: (url: string) => void;
  /** Below the last row: what the run is doing now. */
  footer?: React.ReactNode;
}) {
  const listRef = useRef<LegendListRef>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const [following, setFollowing] = useState(true);
  const userIntentAt = useRef(0);
  const live = timeline.status === "running";
  const items = timeline.items as Row[];

  // Collapsed activity groups: the newest one stays open while a turn runs.
  const [toggled, setToggled] = useState<ReadonlySet<string>>(() => new Set());
  const lastActivity = live ? items.findLast((i) => i.kind === "activity" && i.turn === items[items.length - 1]?.turn)?.id : undefined;
  const isOpen = useCallback((id: string) => (id === lastActivity) !== toggled.has(id), [lastActivity, toggled]);
  const toggle = useCallback(
    (id: string) =>
      setToggled((prev) => {
        const next = new Set(prev);
        if (next.has(id)) next.delete(id);
        else next.add(id);
        return next;
      }),
    [],
  );

  useEffect(() => {
    const el = viewportRef.current;
    if (!el) return;
    const mark = () => {
      userIntentAt.current = performance.now();
    };
    const onKey = (e: KeyboardEvent) => {
      if (["ArrowUp", "PageUp", "Home", "ArrowDown", "PageDown", "End", " "].includes(e.key)) mark();
    };
    el.addEventListener("wheel", mark, { passive: true });
    el.addEventListener("touchmove", mark, { passive: true });
    el.addEventListener("pointerdown", mark);
    el.addEventListener("keydown", onKey);
    return () => {
      el.removeEventListener("wheel", mark);
      el.removeEventListener("touchmove", mark);
      el.removeEventListener("pointerdown", mark);
      el.removeEventListener("keydown", onKey);
    };
  }, []);

  const onScroll = useCallback(() => {
    const s = listRef.current?.getState?.();
    if (!s) return;
    const atEnd = s.scroll + s.scrollLength >= s.contentLength - END_SLOP;
    if (atEnd) setFollowing(true);
    // Only the reader lets go of the end; growth that outpaces a scroll does not.
    else if (performance.now() - userIntentAt.current < INTENT_MS) setFollowing(false);
  }, []);

  // Follow the end ourselves as well: LegendList's maintainScrollAtEnd does
  // not re-pin on every growth on web, and a message grows a chunk at a time.
  const followingRef = useRef(following);
  followingRef.current = following;
  const pinFrame = useRef<number | null>(null);
  const pin = useCallback(() => {
    if (!followingRef.current || pinFrame.current !== null) return;
    pinFrame.current = requestAnimationFrame(() => {
      pinFrame.current = null;
      if (followingRef.current) void listRef.current?.scrollToEnd({ animated: false });
    });
  }, []);
  useEffect(() => () => {
    if (pinFrame.current !== null) cancelAnimationFrame(pinFrame.current);
  }, []);
  useEffect(pin, [items, timeline.status, timeline.phase, toggled, pin]);

  const jumpToEnd = () => {
    setFollowing(true);
    void listRef.current?.scrollToEnd({ animated: true });
  };

  const lastId = items[items.length - 1]?.id;
  const renderItem = useCallback(
    ({ item }: { item: Row }) => (
      <TimelineRow
        item={item}
        streaming={live && item.id === lastId && item.kind === "message"}
        open={item.kind === "activity" ? isOpen(item.id) : false}
        onToggle={toggle}
        onOpenLink={onOpenLink}
      />
    ),
    [isOpen, lastId, live, onOpenLink, toggle],
  );

  const listFooter = useMemo(() => <div className="mx-auto w-full max-w-[720px] px-8 pt-1 pb-8">{footer}</div>, [footer]);

  return (
    <div ref={viewportRef} className={cn("relative min-h-0", className)}>
      <LegendList<Row>
        ref={listRef}
        data={items}
        keyExtractor={(row) => row.id}
        getItemType={(row) => row.kind}
        renderItem={renderItem}
        extraData={`${live}:${lastActivity}:${toggled.size}`}
        estimatedItemSize={72}
        initialScrollAtEnd
        maintainScrollAtEnd={following ? FOLLOW : false}
        maintainScrollAtEndThreshold={1}
        maintainVisibleContentPosition={{ data: true, size: true }}
        onScroll={onScroll}
        onItemSizeChanged={pin}
        recycleItems={false}
        className="h-full overscroll-y-contain [overflow-anchor:none]"
        ListHeaderComponent={<div className="h-6" />}
        ListFooterComponent={listFooter}
        aria-label="Run timeline"
        tabIndex={0}
      />
      {!following ? (
        <div className="pointer-events-none absolute inset-x-0 bottom-4 flex justify-center">
          <Button size="sm" variant="outline" className="pointer-events-auto rounded-full shadow-sm" onClick={jumpToEnd}>
            <ArrowDownIcon className="size-3.5" />
            {live ? "Follow the run" : "Jump to latest"}
          </Button>
        </div>
      ) : null}
    </div>
  );
}

const TimelineRow = memo(function TimelineRow({
  item,
  streaming,
  open,
  onToggle,
  onOpenLink,
}: {
  item: Row;
  streaming: boolean;
  open: boolean;
  onToggle: (id: string) => void;
  onOpenLink?: (url: string) => void;
}) {
  return (
    <div className="mx-auto w-full max-w-[720px] px-8 py-2" data-kind={item.kind}>
      {item.kind === "user" ? (
        <div className="flex justify-end">
          <div className="max-w-[85%] rounded-2xl rounded-br-md bg-muted px-3.5 py-2 text-[13px] leading-[1.55] whitespace-pre-wrap">{item.text}</div>
        </div>
      ) : item.kind === "message" ? (
        <div className={cn(streaming && "markdown-streaming")}>
          <Markdown text={item.text} onOpenLink={onOpenLink} />
        </div>
      ) : (
        <ActivityGroup item={item} open={open} onToggle={onToggle} />
      )}
    </div>
  );
});

function ActivityGroup({ item, open, onToggle }: { item: Row; open: boolean; onToggle: (id: string) => void }) {
  const last = item.steps[item.steps.length - 1];
  return (
    <div className="text-xs text-muted-foreground">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => onToggle(item.id)}
        className="-mx-1.5 flex max-w-full cursor-default items-center gap-1.5 rounded-md px-1.5 py-0.5 text-left outline-none hover:bg-foreground/[0.04] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
      >
        <ChevronRightIcon className={cn("size-3 shrink-0 transition-transform duration-150", open && "rotate-90")} />
        <span className="shrink-0 font-medium">{item.text}</span>
        {!open && last ? <span className="truncate text-muted-foreground/80">{last.text}</span> : null}
      </button>
      {open ? (
        <ol className="mt-1 ml-[5px] space-y-0.5 border-l pl-3.5">
          {item.steps.map((step) => (
            <li key={step.id} className={cn("truncate font-mono text-[11.5px] leading-[1.6]", step.failed && "text-destructive")} title={step.text}>
              {step.text}
            </li>
          ))}
        </ol>
      ) : null}
    </div>
  );
}
