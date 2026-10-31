// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, FileTextIcon, ImageIcon, ReplyIcon, XIcon } from "lucide-react";
import { Fragment, memo, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";

import { useBridge, type ThreadAttachment, type ThreadEntry, type ThreadItem, type ThreadQuote, type ThreadView } from "@/bridge";
import { Inlines } from "@/components/markdown";
import { Button } from "@/components/ui/button";
import { parseInline } from "@/lib/markdown";
import { mentionSegments } from "@/lib/mentions";
import { cn, isMacPlatform } from "@/lib/utils";

import { AgentTile, MemberAvatar } from "./avatar";
import { FailureNote } from "./failure-note";
import { AgentName, AiTag, holderNames, MemberEventNote } from "./identity";
import { MentionChip, ProfileTrigger } from "./profile-card";
import { waitingFor } from "./rights";
import type { Roster } from "./roster";

/**
 * Keeps `scroller` at its end while `content` grows, as long as `atEnd()`
 * (you hadn't scrolled up). Returns the stop.
 */
export function followGrowth(scroller: HTMLElement, content: HTMLElement, atEnd: () => boolean): () => void {
  if (typeof ResizeObserver === "undefined") return () => {};
  const ro = new ResizeObserver(() => {
    if (atEnd()) scroller.scrollTop = scroller.scrollHeight;
  });
  ro.observe(content);
  return () => ro.disconnect();
}

/**
 * The Space thread under the desktop: messages, tasks with their status and
 * the agent's replies nested under them, approvals, one-line notes and the
 * unread divider (`foldThread`). It opens at the divider when there is
 * one, else at the end, and follows new entries while you are at the end.
 */
export function ThreadView({
  items,
  roster,
  onAnswer,
  onOpenAttachment,
  onReachTop,
  onRemoveAgent,
  onRetry,
  onReply,
  empty,
  className,
  endInset = 0,
  after,
  tail,
}: {
  items: ThreadItem[];
  /** Room kept clear at the end (px): thread mode's mini-player sits there, never over the newest message. */
  endInset?: number;
  /** Drawn under the item that posted entry `seq` (a summon's "can't be woken" note). */
  after?: (seq: number) => ReactNode;
  /** Drawn after the last item. */
  tail?: ReactNode;
  /** Removes an agent member from its card (absent: the host can't). */
  onRemoveAgent?: (handle: string) => Promise<void>;
  /** Sends a message again (a failed agent's Retry); absent: no Retry. */
  onRetry?: (text: string) => Promise<void>;
  /** Reply on an agent's message: puts its @handle in the composer (absent: no Reply button). */
  onReply?: (handle: string, seq: number) => void;
  /** Scrolled to the top: read the page before (when there is one). */
  onReachTop?: () => Promise<void>;
  roster: Roster;
  onAnswer: (approval: number, approve: boolean) => Promise<void>;
  onOpenAttachment: (a: ThreadAttachment) => void;
  /** What shows before anyone has written. */
  empty: ReactNode;
  className?: string;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const atEnd = useRef(true);
  const opened = useRef(false);
  // Older entries read in above: keep what you were looking at in place.
  const keep = useRef<number | null>(null);
  const first = items.length ? items[0]!.key : "";
  const last = items.length ? items[items.length - 1]!.key : "";
  // Your own message just went in: the thread follows it to the end, wherever you were reading.
  const lastItem = items.length ? items[items.length - 1]! : null;
  const mineLast = Boolean(lastItem && "entry" in lastItem && roster.people.some((p) => p.me && p.id === lastItem.entry.author.id));
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || !items.length) return;
    if (mineLast && opened.current) atEnd.current = true;
    if (keep.current !== null) {
      el.scrollTop += el.scrollHeight - keep.current;
      keep.current = null;
      return;
    }
    if (!opened.current) {
      opened.current = true;
      // Opens with the unread divider about a third of the way down, so what
      // you read last is just above it and the new entries follow.
      const divider = el.querySelector<HTMLElement>("[data-unread]");
      if (divider) {
        el.scrollTop = Math.max(0, divider.offsetTop - el.clientHeight / 3);
        atEnd.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
        return;
      }
    }
    if (atEnd.current) el.scrollTop = el.scrollHeight;
    // Follows the first and last items, not every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [first, last, items.length, mineLast]);
  // The mini-player moved to (or grew at) the bottom: keep the newest entry in view above it.
  useLayoutEffect(() => {
    const el = ref.current;
    if (el && atEnd.current) el.scrollTop = el.scrollHeight;
  }, [endInset]);
  // Something under the last entry grew (a note only you see, a status line):
  // when you were at the end, stay there.
  const body = useRef<HTMLDivElement | null>(null);
  const hasItems = items.length > 0;
  useEffect(() => {
    const el = ref.current;
    const inner = body.current;
    if (!el || !inner) return;
    return followGrowth(el, inner, () => atEnd.current && keep.current === null);
  }, [hasItems]);
  return (
    <div
      ref={ref}
      data-thread=""
      role="log"
      aria-label="Thread"
      onScroll={(e) => {
        const el = e.currentTarget;
        atEnd.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
        if (el.scrollTop < 80 && onReachTop && keep.current === null) {
          keep.current = el.scrollHeight;
          void onReachTop().then(() => {
            // Nothing came in: the next scroll to the top may ask again.
            if (ref.current && keep.current === ref.current.scrollHeight) keep.current = null;
          });
        }
      }}
      // Soft fades at the edges: an entry cut off there reads as scrolled, not broken.
      className={cn("relative overflow-y-auto overscroll-contain [mask-image:linear-gradient(to_bottom,transparent_0,black_28px,black_calc(100%-16px),transparent_100%)]", className)}
    >
      {items.length === 0 ? (
        <div className="flex h-full items-end px-8 pb-4">{empty}</div>
      ) : (
        <div ref={body} className="flex min-h-full flex-col justify-end gap-1 px-8 pt-5 pb-4" style={endInset ? { paddingBottom: endInset + 16 } : undefined}>
          {items.map((item, i) => (
            <Fragment key={item.key}>
              <ThreadItemView item={item} above={aboveSeq(items, i)} roster={roster} onAnswer={onAnswer} onOpenAttachment={onOpenAttachment} onRemoveAgent={onRemoveAgent} onRetry={onRetry} onReply={onReply} />
              {after && "entry" in item ? after(item.entry.seq) : null}
            </Fragment>
          ))}
          {tail}
        </div>
      )}
    </div>
  );
}

/** The seq of the message drawn right above `items[i]` (notes and the unread divider are not messages). */
export function aboveSeq(items: readonly ThreadItem[], i: number): number | null {
  for (let j = i - 1; j >= 0; j--) {
    const it = items[j]!;
    if (it.type === "system" || it.type === "unread") continue;
    return it.entry.seq;
  }
  return null;
}

/** One folded thread item as the thread draws it; the Agents tab's DM draws the same, labelled with its Space. */
export const ThreadItemView = memo(function Item({
  item,
  roster,
  onAnswer,
  onOpenAttachment,
  onRemoveAgent,
  onRetry,
  onReply,
  label,
  above = null,
}: {
  item: ThreadItem;
  roster: Roster;
  /** The seq of the entry drawn right above this one. */
  above?: number | null;
  onAnswer: (approval: number, approve: boolean) => Promise<void>;
  onOpenAttachment: (a: ThreadAttachment) => void;
  onRemoveAgent?: (handle: string) => Promise<void>;
  onRetry?: (text: string) => Promise<void>;
  onReply?: (handle: string, seq: number) => void;
  /** Beside each message's time (the DM's Space). */
  label?: ReactNode;
}) {
  const m = { roster, onOpenAttachment, onRemoveAgent, onReply, label };
  switch (item.type) {
    case "unread":
      return (
        <div data-unread="" className="my-2 flex items-center gap-2 text-[11px] font-medium text-brand-strong">
          <span className="h-px flex-1 bg-brand/40" />
          New
        </div>
      );
    case "system":
      // A join or a removal with who it was: drawn with its mark; anything else is its line of text.
      if (item.entry.event?.member) return <MemberEventNote event={item.entry.event} text={item.text} tsMs={item.entry.tsMs} />;
      return (
        <p data-thread-note={item.entry.seq} className="my-1 text-center text-xs break-words text-muted-foreground">
          {item.text}
        </p>
      );
    case "message":
      // One row, in order. A reply quotes the message it answers only when that is not the one right above it.
      return <Message entry={item.entry} {...m} quote={item.quote && item.quote.seq !== above ? item.quote : null} task={item.taskText !== null ? { text: item.taskText, onRetry } : undefined} />;
    case "task":
      // How the task is going is told by the chat: the live row, the agent's reply or its failure message. No chip here.
      return (
        <div data-task={item.entry.seq} data-task-state={item.state}>
          <Message entry={item.entry} {...m} footnote={keysLine(item.entry, roster)} />
        </div>
      );
    case "join_request":
      return <JoinRequestCard item={item} roster={roster} onAnswer={onAnswer} />;
    case "approval":
      return <Approval item={item} roster={roster} onAnswer={onAnswer} onOpenAttachment={onOpenAttachment} onRemoveAgent={onRemoveAgent} label={label} />;
  }
});

/** "Runs on Ada's keys" under a task someone else gave an agent that uses the owner's provider keys. */
function keysLine(entry: ThreadEntry, roster: Roster): string | null {
  const k = entry.runsOnKeysOf;
  if (!k) return null;
  return roster.people.some((p) => p.me && p.id === k.id) ? "Runs on your keys" : `Runs on ${k.name}'s keys`;
}

function Message({
  entry,
  roster,
  onOpenAttachment,
  onRemoveAgent,
  onReply,
  label,
  status,
  footnote,
  quote,
  task,
  surface = "background",
}: {
  entry: ThreadEntry;
  roster: Roster;
  onOpenAttachment: (a: ThreadAttachment) => void;
  onRemoveAgent?: (handle: string) => Promise<void>;
  onReply?: (handle: string, seq: number) => void;
  label?: ReactNode;
  /** What the avatar sits on (the owner badge is cut out of it). */
  surface?: "background" | "muted";
  /** Beside the name and time (a task's state, an approval's). */
  status?: ReactNode;
  /** A muted line under the text ("Runs on Ada's keys"). */
  footnote?: string | null;
  /** The message this one replies to, when it is not the one right above. */
  quote?: ThreadQuote | null;
  /** The task this message replies to (its text, for a failed reply's Retry). */
  task?: { text: string; onRetry?: (text: string) => Promise<void> };
}) {
  const who = roster.author(entry.author);
  // A person's card: the roster's entry for this author (by id, else by name).
  const person = who.kind === "person" ? (roster.people.find((p) => p.id === entry.author.id) ?? roster.people.find((p) => p.label === who.label)) : undefined;
  const avatar = (
    <MemberAvatar
      kind={who.kind === "agent" ? "agent" : "person"}
      name={who.kind === "agent" ? (who.handle ?? who.label) : who.label}
      harness={who.harness}
      identity={who.identity}
      surface={surface}
      size={28}
      className="mt-0.5"
    />
  );
  // Reply: an agent still in the Space can be addressed from its message.
  const replyTo = who.kind === "agent" && onReply ? who.agent?.handle : undefined;
  return (
    <div
      data-entry={entry.seq}
      className="group/msg relative flex gap-3 py-2"
      // R on a focused message (or its Reply button) replies, as the button does.
      onKeyDown={
        replyTo
          ? (e) => {
              if (e.key.toLowerCase() !== "r" || e.metaKey || e.ctrlKey || e.altKey || e.shiftKey || e.nativeEvent.isComposing) return;
              const t = e.target as HTMLElement;
              if (t.closest("input, textarea, [contenteditable=true]")) return;
              e.preventDefault();
              onReply!(replyTo, entry.seq);
            }
          : undefined
      }
    >
      {replyTo ? (
        <button
          type="button"
          data-reply={replyTo}
          aria-label={`Reply to ${who.label}`}
          title={`Reply to ${who.label} (R, or ${isMacPlatform() ? "⌘⇧A" : "Ctrl+Shift+A"} for the latest)`}
          onClick={() => onReply!(replyTo, entry.seq)}
          className="absolute top-1 right-0 inline-flex size-7 cursor-default items-center justify-center rounded-lg bg-background text-muted-foreground opacity-0 shadow-sm outline-none ring-1 ring-border transition-opacity group-focus-within/msg:opacity-100 group-hover/msg:opacity-100 hover:bg-accent hover:text-foreground focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-ring/60 max-md:opacity-100 [@media(hover:none)]:opacity-100 [@media(pointer:coarse)]:size-9"
        >
          <ReplyIcon className="size-3.5" />
        </button>
      ) : null}
      {who.kind === "agent" && who.identity ? (
        <ProfileTrigger identity={who.identity} agent={who.agent} roster={roster} onRemove={onRemoveAgent} className="self-start rounded-full">
          {avatar}
        </ProfileTrigger>
      ) : person ? (
        <ProfileTrigger person={person} className="self-start rounded-full">
          {avatar}
        </ProfileTrigger>
      ) : (
        avatar
      )}
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          {who.kind === "agent" ? <AgentName who={who} roster={roster} onRemove={onRemoveAgent} /> : person ? (
            <ProfileTrigger person={person} className="truncate text-[13px] font-semibold hover:underline" data={{ "data-person-name": person.label }}>
              {who.label}
            </ProfileTrigger>
          ) : (
            <span className="truncate text-[13px] font-semibold">{who.label}</span>
          )}
          <time className="shrink-0 text-[11px] text-muted-foreground tabular-nums" dateTime={new Date(entry.tsMs).toISOString()} title={new Date(entry.tsMs).toLocaleString()}>
            {clock(entry.tsMs)}
          </time>
          {label ? <span className="shrink-0">{label}</span> : null}
          {status ? <span className="shrink-0">{status}</span> : null}
        </div>
        {quote ? <ReplyQuote quote={quote} roster={roster} /> : null}
        {entry.text ? <ThreadText text={entry.text} handles={roster.handles} /> : null}
        {entry.failure ? <FailureNote failure={entry.failure} harness={who.harness ?? ""} taskText={task?.text ?? null} onRetry={task?.onRetry} /> : null}
        {entry.view ? <ViewLine view={entry.view} /> : null}
        {footnote ? (
          <p data-entry-footnote="" className="mt-0.5 text-[11px] text-muted-foreground">
            {footnote}
          </p>
        ) : null}
        {entry.attachments.length ? (
          <div className="mt-2 flex flex-wrap gap-2">
            {entry.attachments.map((a) => (
              <AttachmentTile key={a.path} a={a} onOpen={() => onOpenAttachment(a)} />
            ))}
          </div>
        ) : null}
      </div>
    </div>
  );
}

/** "Replying to Ada: ...": the message a reply answers, one quiet line (a preview of another message, not an error). */
function ReplyQuote({ quote, roster }: { quote: ThreadQuote; roster: Roster }) {
  const who = roster.author(quote.author);
  const text = quote.text.replace(/\s+/g, " ").trim();
  return (
    <p data-reply-quote={quote.seq} className="mb-0.5 truncate border-l-2 pl-2 text-[11.5px] text-muted-foreground">
      <span className="font-medium">{who.label}</span> {text}
    </p>
  );
}

/** What the person was looking at as they posted, one quiet line ("Sent from Firefox"); the core's words (`stream.viewLine`). */
function ViewLine({ view }: { view: ThreadView }) {
  const { core } = useBridge();
  const line = core.tryCall<string>("stream.viewLine", { kind: view.kind, app: view.app, title: view.title }) ?? null;
  if (!line) return null;
  return (
    <p data-entry-view={view.kind} title={view.title ?? undefined} className="mt-0.5 truncate text-[11px] text-muted-foreground">
      {line}
    </p>
  );
}

/** A message's text: inline Markdown, `@name` of a member as a chip. */
export function ThreadText({ text, handles }: { text: string; handles: ReadonlySet<string> }) {
  const chips = (t: string, key: number) =>
    mentionSegments(t, handles).map((s, i) =>
      s.mention ? (
        <MentionChip key={`${key}-${i}`} handle={s.handle}>
          {s.text}
        </MentionChip>
      ) : (
        s.text
      ),
    );
  return (
    <div className="text-[13px] leading-[1.55] break-words text-foreground select-text">
      {text.split("\n").map((line, i) => (
        <p key={i} className="min-h-[1.55em]">
          <Inlines nodes={parseInline(line)} renderText={chips} />
        </p>
      ))}
    </div>
  );
}

function AttachmentTile({ a, onOpen }: { a: ThreadAttachment; onOpen: () => void }) {
  const image = a.mime?.startsWith("image/") ?? false;
  const Icon = image ? ImageIcon : FileTextIcon;
  return (
    <button
      type="button"
      data-attachment={a.name}
      title={a.path}
      onClick={onOpen}
      className="flex min-w-36 cursor-default items-center gap-2.5 rounded-lg bg-muted/70 px-3 py-2 text-left outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring/60"
    >
      <Icon className="size-4 shrink-0 text-muted-foreground" />
      <span className="min-w-0">
        <span className="block max-w-48 truncate text-[12.5px] font-medium">{a.name}</span>
        <span className="block text-[11px] text-muted-foreground">{a.size != null ? bytes(a.size) : image ? "Image" : fileKind(a)}</span>
      </span>
    </button>
  );
}

function Approval({
  item,
  roster,
  onAnswer,
  onOpenAttachment,
  onRemoveAgent,
  label,
}: {
  item: Extract<ThreadItem, { type: "approval" }>;
  roster: Roster;
  onAnswer: (approval: number, approve: boolean) => Promise<void>;
  onOpenAttachment: (a: ThreadAttachment) => void;
  onRemoveAgent?: (handle: string) => Promise<void>;
  label?: ReactNode;
}) {
  const [busy, setBusy] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { entry, answer } = item;
  const closed = answer !== null || (entry.approval?.state ?? "open") !== "open";
  // Only who holds the asking agent's `approve` right answers (the daemon refuses anyone else).
  const asker = entry.author.kind === "agent" ? roster.agents.find((a) => a.id === entry.author.id) : undefined;
  const canAnswer = asker ? asker.youCan.approve : true;
  const press = (approve: boolean) => {
    setBusy(approve);
    setError(null);
    onAnswer(entry.seq, approve).then(
      () => setBusy(null),
      (e: unknown) => {
        setBusy(null);
        setError(e instanceof Error ? e.message : String(e));
      },
    );
  };
  return (
    <div data-approval={entry.seq} data-approval-state={answer ? (answer.approve ? "approved" : "denied") : (entry.approval?.state ?? "open")} className="-mx-3 my-1.5 rounded-lg bg-muted/50 px-3 pb-3">
      <Message
        entry={entry}
        roster={roster}
        onOpenAttachment={onOpenAttachment}
        onRemoveAgent={onRemoveAgent}
        label={label}
        surface="muted"
        status={
          closed ? null : (
            <span className="inline-flex items-center gap-1.5 text-[11px] text-muted-foreground">
              <span aria-hidden className="size-1.5 rounded-full bg-warning" />
              Needs approval
            </span>
          )
        }
      />
      <div className="ml-10">
        <p className="text-[13px] font-medium">{entry.approval?.action}</p>
        {entry.approval?.details ? <p className="mt-0.5 text-xs text-muted-foreground">{entry.approval.details}</p> : null}
      </div>
      <div className="mt-2.5 ml-10 flex items-center gap-2">
        {answer ? (
          <p data-approval-answer="" className="flex items-center gap-1.5 text-xs text-muted-foreground">
            {answer.approve ? <CheckIcon className="size-3.5 text-success" /> : <XIcon className="size-3.5 text-destructive" />}
            <span title={new Date(answer.tsMs).toLocaleString()}>
              {answer.approve ? "Approved" : "Denied"} by {roster.author(answer.by).label}
            </span>
          </p>
        ) : closed ? (
          <p className="text-xs text-muted-foreground">{capital(entry.approval?.state ?? "")}</p>
        ) : !canAnswer && asker ? (
          <p data-approval-waiting="" className="text-xs text-muted-foreground">
            {Array.isArray(asker.rights.approve) && holderNames(asker.rights.approve, asker.identity, roster)
              ? `Waiting for ${holderNames(asker.rights.approve, asker.identity, roster)}`
              : waitingFor(asker.rights.approve)}
          </p>
        ) : (
          <>
            <Button size="sm" disabled={busy !== null} onClick={() => press(true)}>
              Approve
            </Button>
            <Button size="sm" variant="outline" disabled={busy !== null} onClick={() => press(false)}>
              Deny
            </Button>
            {error ? <span className="text-xs text-destructive">{error}</span> : null}
          </>
        )}
      </div>
    </div>
  );
}

/**
 * Someone's agent redeemed a code and asks to join (p. 18): who connected
 * it and from where, what it will do, and Let it join / Deny for the owner
 * and admins. Answered like an approval.
 */
function JoinRequestCard({ item, roster, onAnswer }: { item: Extract<ThreadItem, { type: "join_request" }>; roster: Roster; onAnswer: (approval: number, approve: boolean) => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { entry, answer } = item;
  const req = entry.joinRequest!;
  const i = req.member;
  const state = answer ? (answer.approve ? "approved" : "denied") : req.state;
  // The daemon says who may answer; an older one doesn't, and then the owner and admins may.
  const canAnswer = req.youCanAnswer ?? roster.admin;
  const press = (approve: boolean) => {
    setBusy(true);
    setError(null);
    onAnswer(entry.seq, approve).then(
      () => setBusy(false),
      (e: unknown) => {
        setBusy(false);
        setError(e instanceof Error ? e.message : String(e));
      },
    );
  };
  const name = i?.displayName ?? entry.author.name;
  return (
    <div data-join-request={entry.seq} data-join-request-state={state} className="-mx-3 my-1.5 flex gap-3 rounded-lg bg-muted/50 px-3 py-2.5">
      {i ? <AgentTile harness={i.harness.id} icon={i.icon} owner={i.owner} name={i.handle} px={28} surface="muted" className="mt-0.5" /> : <MemberAvatar kind="agent" name={name} size={28} className="mt-0.5" />}
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate text-[13px] font-semibold">{name}</span>
          <AiTag />
          <time className="shrink-0 text-[11px] text-muted-foreground tabular-nums" dateTime={new Date(entry.tsMs).toISOString()} title={new Date(entry.tsMs).toLocaleString()}>
            {clock(entry.tsMs)}
          </time>
          {state === "open" ? (
            <span className="inline-flex items-center gap-1.5 text-[11px] text-muted-foreground">
              <span aria-hidden className="size-1.5 rounded-full bg-warning" />
              Wants to join
            </span>
          ) : null}
        </div>
        <p className="text-[13px] leading-[1.55]">
          {i
            ? `${i.owner.name} connected ${i.displayName}${i.device ? ` from ${i.device.name}` : ""}. It will read this thread and take tasks from ${req.canTask === "owner" ? i.owner.name : "members"}.`
            : entry.text}
        </p>
        <div className="mt-2.5 flex items-center gap-2">
          {answer ? (
            <p data-join-request-answer="" className="flex items-center gap-1.5 text-xs text-muted-foreground">
              {answer.approve ? <CheckIcon className="size-3.5 text-success" /> : <XIcon className="size-3.5 text-destructive" />}
              {answer.approve ? `${roster.author(answer.by).label} let it join` : `${roster.author(answer.by).label} said no`}
            </p>
          ) : state !== "open" ? (
            <p className="text-xs text-muted-foreground">{state === "expired" ? "The request was withdrawn" : capital(state)}</p>
          ) : canAnswer ? (
            <>
              <Button size="sm" data-join-request-allow="" disabled={busy} onClick={() => press(true)}>
                Let it join
              </Button>
              <Button size="sm" variant="secondary" data-join-request-deny="" disabled={busy} onClick={() => press(false)}>
                Deny
              </Button>
              {error ? <span className="text-xs text-destructive">{error}</span> : null}
            </>
          ) : (
            <p className="text-xs text-muted-foreground">Waiting for the owner or an admin</p>
          )}
        </div>
      </div>
    </div>
  );
}

const capital = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** `10:41` today; `Yesterday`, a weekday or a date before that (hover shows the full time). */
export function clock(tsMs: number, now = Date.now()): string {
  const d = new Date(tsMs);
  const today = new Date(now);
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((day(today) - day(d)) / 86_400_000);
  if (days <= 0) return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (days === 1) return "Yesterday";
  if (days < 7) return d.toLocaleDateString([], { weekday: "long" });
  return d.toLocaleDateString([], { month: "short", day: "numeric" });
}

function fileKind(a: ThreadAttachment): string {
  const ext = a.name.includes(".") ? a.name.slice(a.name.lastIndexOf(".") + 1).toUpperCase() : "";
  return ext || "File";
}

function bytes(n: number): string {
  if (n < 1000) return `${n} B`;
  if (n < 1_000_000) return `${Math.round(n / 1000)} KB`;
  return `${(n / 1_000_000).toFixed(1)} MB`;
}
