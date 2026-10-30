// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { XIcon } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { Minimize2Icon } from "lucide-react";

import { Tooltip } from "@/components/ui/tooltip";
import { isMacPlatform } from "@/lib/utils";

import { harnessName, queuedNote, summonPlan, useBridge, type SpaceFact, type SpaceThread, type StreamRow, type ThreadAttachment, type ThreadView as PostView } from "@/bridge";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { toast } from "@/components/ui/toast";

import { AddAgentSheet } from "@/components/add-agent/add-agent-sheet";
import { registerAddAgent } from "@/components/add-agent/add-agent-launcher";

import { CallSplit } from "./call-split";
import { defaultAddressee, EMPTY_ADDRESS, resolveAddressing, setAddress, unpinAgent, useAddressing } from "@/lib/addressing";
import { EVERYONE, type MentionCandidate } from "@/lib/mentions";

import { Composer, type ComposerHandle } from "./composer";
import { LiveRows } from "./live-row";
import { MembersPanel, type MemberPresence } from "./members-panel";
import { panelFacts } from "./rail";
import { useStreamHidden, type SpaceLayout } from "./layout-mode";
import { MiniPlayer } from "./mini-player";
import { GUEST_COMPOSER } from "./rights";
import { buildRoster } from "./roster";
import { dismissWakeNote, OnlyYou, useSummon, useWakeNotes, WakeNoteView } from "./summon";
import { ProfileScopeProvider } from "./profile-card";
import { queuedItems, useQueued } from "./queued";
import { ThreadView } from "./thread-view";


/** Why `@everyone` is off for someone who can't give tasks. */
const EVERYONE_REASON = "Only people who can give tasks can message everyone";

/**
 * A Space as a call (the call storyboard): the header, the desktop on top
 * with the thread below it, and who is in the Space on the right with the
 * details under them. More opens everything the classic page lists.
 * `presence` is P2's seam for who is here now; nothing draws without it.
 */
export function SpaceCall({
  spaceId,
  spaceName,
  header,
  notice,
  stage,
  thread,
  identity,
  facts,
  more,
  windows,
  desktop,
  selected,
  onSelect,
  view,
  onStream,
  presence,
  onShare,
  layout = "call",
  onLayout,
  smallStage,
  aspect = 16 / 10,
  stageLabel = "Desktop",
  creating = false,
}: {
  /** Call mode (the desktop is the main area) or thread mode (the thread is, and the desktop shrinks). */
  layout?: SpaceLayout;
  onLayout?: (next: SpaceLayout) => void;
  /** The desktop for thread mode: view only, `h-full` (the same live session as the stage). */
  smallStage?: ReactNode;
  /** The desktop's aspect ratio (the mini-player keeps it). */
  aspect?: number;
  /** What the mini-player's bar says ("Desktop", or the window shown). */
  stageLabel?: string;
  /** The Space's id (Add an agent joins agents to it). */
  spaceId: string;
  spaceName: string;
  header: ReactNode;
  /** A line under the header (a failed power change). */
  notice?: ReactNode;
  stage: ReactNode;
  thread: SpaceThread;
  identity: string | null;
  facts: readonly SpaceFact[];
  /** The classic page's sections (resources, all facts, Teleport, windows, runs). */
  more: ReactNode;
  /** The Space's windows (Stream rows without the desktop). */
  windows: StreamRow[];
  /** The Desktop row (the default view), first in the Stream list. */
  desktop?: StreamRow | null;
  /** The row the stage shows (the desktop when none). */
  selected?: string | null;
  /** Shows a row on the stage. */
  onSelect?: (row: StreamRow) => void;
  /** What a post says the person is looking at (the stage's window), from the core (`stream.viewContext`). */
  view?: PostView | null;
  /** Opens a row in its own window. */
  onStream: (row: StreamRow) => void;
  presence?: MemberPresence;
  /** Opens Share for this Space (Add an agent offers it; absent: sharing isn't offered here). */
  onShare?: () => void;
  /** The Space is still being made: the composer queues, nothing joins yet (`thread` is `creatingThread`). */
  creating?: boolean;
}) {
  const { core, data } = useBridge();
  const [folded, setFolded] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  const roster = useMemo(
    () => buildRoster({ members: thread.members, shares: thread.shares, entries: thread.entries, me: thread.me, identity, role: thread.role, harnessName: (h) => harnessName(core, h) }),
    [thread.members, thread.shares, thread.entries, thread.me, thread.role, identity, core],
  );
  // Anyone who can post can add an agent; a guest only reads.
  const canAdd = !creating && thread.status === "ready" && thread.role !== "guest";
  // Share's "Add an agent" opens this page's sheet for this Space.
  useEffect(() => (canAdd ? registerAddAgent(spaceId, () => setAddOpen(true)) : undefined), [canAdd, spaceId]);
  const [reserve, setReserve] = useState(0);
  const threadMode = layout === "thread" && Boolean(smallStage);
  // Clicking the already-selected stream hides the stream view (thread only); again shows it.
  const [streamHidden, setStreamHidden] = useStreamHidden(spaceId);
  // ⌘⇧T (Ctrl+Shift+T): thread view and back.
  useEffect(() => {
    if (!onLayout) return;
    const onKey = (e: KeyboardEvent) => {
      const mod = isMacPlatform() ? e.metaKey : e.ctrlKey;
      if (mod && e.shiftKey && !e.altKey && e.key.toLowerCase() === "t") {
        e.preventDefault();
        onLayout(layout === "thread" ? "call" : "thread");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [layout, onLayout]);
  // @ any agent: those in the Space and those that can start in it; sending summons them (`summon.*`).
  const summon = useSummon({ spaceId, thread, roster, canAdd: canAdd || creating });
  const notes = useWakeNotes(spaceId);
  const shown = useMemo(() => ({ ...roster, handles: summon.handles }), [roster, summon.handles]);
  const noteView = (n: NonNullable<typeof notes>[number]) => (
    <WakeNoteView
      key={n.id}
      note={n}
      connected={(summonPlan(core, { members: [], people: [], canAdd: true, connectors: summon.connectors.input }, `@${n.cant.handle}`) ?? []).every((s) => s.action !== "blocked")}
      onConnect={() => void summon.connectAndRetry(n, summon.connectors)}
      onRetry={() => void summon.retry(n)}
      onDismiss={() => dismissWakeNote(spaceId, n.id)}
    />
  );
  // Messages written while the Space was being made: queued until it's ready, then sent in order.
  const queued = useQueued(spaceId);
  const myName = roster.people.find((p) => p.me)?.label ?? "";
  const queuedLine = queuedNote(core, creating);
  const items = useMemo(() => [...thread.items, ...queuedItems(queued, thread.me ?? "me", myName)], [thread.items, thread.me, queued, myName]);
  const after = (seq: number) =>
    seq < 0 ? (
      <div data-queued-note="" data-only-you="" className="-mt-0.5 mb-1 ml-11 w-fit rounded-lg bg-brand-surface px-2.5 py-1.5">
        <OnlyYou label={queuedLine?.onlyYouLabel ?? ""} />
        <p className="text-xs text-muted-foreground">{queuedLine?.text}</p>
      </div>
    ) : (
      notes?.filter((n) => n.seq === seq).map(noteView)
    );
  // The person and agent cards read the Space from here (names, avatars, mentions, the rail).
  // The Space's owner and admins change agents' rights, and so does an agent's own owner (the daemon decides; a guest only reads).
  const [selectedMember, setSelectedMember] = useState<string | null>(null);
  const scope = {
    spaceId,
    roster: shown,
    shares: thread.shares,
    members: thread.members,
    role: thread.role,
    identity,
    onRemoveAgent: thread.removeAgent ?? undefined,
    onSetRights: thread.role === "guest" ? undefined : thread.setRights,
    selected: selectedMember,
    select: setSelectedMember,
  };
  // Who a message goes to when it names no agent (the chip), and `@everyone`.
  const stored = useAddressing((st) => st.bySpace[spaceId]);
  const address = stored ?? EMPTY_ADDRESS;
  const inSpace = useMemo(() => roster.agents.map((a) => a.handle.toLowerCase()), [roster.agents]);
  const mayTask = thread.role !== "guest" && roster.agents.some((a) => a.youCan.task);
  const everyoneOffered = !creating && roster.agents.length > 1;
  const candidates = useMemo<MentionCandidate[]>(() => {
    if (!everyoneOffered) return summon.candidates;
    const all: MentionCandidate = { handle: EVERYONE, label: "Everyone", kind: "agent", detail: mayTask ? "All agents you can give tasks" : EVERYONE_REASON, disabled: !mayTask };
    const lastAgent = summon.candidates.map((c) => c.kind).lastIndexOf("agent");
    return [...summon.candidates.slice(0, lastAgent + 1), all, ...summon.candidates.slice(lastAgent + 1)];
  }, [summon.candidates, everyoneOffered, mayTask]);
  const known = useMemo(() => (everyoneOffered && mayTask ? new Set([...summon.handles, EVERYONE]) : summon.handles), [summon.handles, everyoneOffered, mayTask]);
  const agentHandles = useMemo(() => new Set([...inSpace, ...summon.candidates.filter((c) => c.kind === "agent").map((c) => c.handle.toLowerCase())]), [inSpace, summon.candidates]);
  const addressee = useMemo(() => defaultAddressee({ agents: inSpace, canEveryone: everyoneOffered && mayTask, state: address }), [inSpace, everyoneOffered, mayTask, address]);
  const nameOf = (h: string) => (h === EVERYONE ? "Everyone" : (roster.agents.find((a) => a.handle.toLowerCase() === h)?.label ?? h));
  const problemOf = (h: string): string | null => {
    const a = roster.agents.find((x) => x.handle.toLowerCase() === h);
    if (!a) return null;
    if (a.status === "failed") return a.failure?.class === "needs_connect" ? `${a.label} needs to be connected` : `${a.label} couldn't start`;
    if (a.identity?.state === "pending") return `${a.label} isn't connected yet`;
    return null;
  };
  // Reply on an agent's message: its @handle goes in the box; the message is posted as a reply to it.
  const composerRef = useRef<ComposerHandle | null>(null);
  const replying = useRef<{ seq: number; handle: string } | null>(null);
  const onReply = useCallback((handle: string, seq: number) => {
    replying.current = { seq, handle: handle.toLowerCase() };
    composerRef.current?.reply(handle);
  }, []);
  // ⌘⇧A (Ctrl+Shift+A): reply to the latest message from an agent that is still here (⌘⇧R is a reload in the desktop app).
  useEffect(() => {
    if (creating) return;
    const onKey = (e: KeyboardEvent) => {
      const mod = isMacPlatform() ? e.metaKey : e.ctrlKey;
      if (!(mod && e.shiftKey && !e.altKey && e.key.toLowerCase() === "a")) return;
      for (let i = thread.items.length - 1; i >= 0; i--) {
        const it = thread.items[i]!;
        if (!("entry" in it) || it.entry.author.kind !== "agent") continue;
        const handle = roster.author(it.entry.author).agent?.handle;
        if (!handle) continue;
        e.preventDefault();
        onReply(handle, it.entry.seq);
        return;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [creating, thread.items, roster, onReply]);
  const sendResolved = async (text: string) => {
    const reply = replying.current;
    const r = resolveAddressing({ text, agents: inSpace, agentHandles, known, canEveryone: everyoneOffered && mayTask, state: address, replyTo: reply?.handle });
    const replyTo = reply && r.addressee.handles.includes(reply.handle) ? reply.seq : null;
    await summon.send(r.text, view ?? null, replyTo);
    replying.current = null;
    // The first time a message is addressed for you, say so quietly.
    const told = address.told || r.auto;
    setAddress(spaceId, { ...r.next, told });
    if (r.auto && !address.told) toast(`${r.addressee.handles.map(nameOf).join(" and ")} will be mentioned automatically`, { description: "Click the X on the name in the box to stop." });
  };
  const harnessOf = (handle: string) => roster.agents.find((a) => a.name === handle)?.harness ?? summon.candidates.find((c) => c.handle === handle)?.harness ?? "";
  // Attachments are Cua Volume paths: show the file where the Volume is mounted.
  const openAttachment = (a: ThreadAttachment) => {
    if (!data) return;
    data.call("volume.reveal", { path: a.path }).catch((e: unknown) =>
      toast(`Couldn't show ${a.name}`, { description: e instanceof Error ? e.message : String(e), type: "error" }),
    );
  };
  const threadView = (endInset: number) => (
    <ThreadView
      className="min-h-0 flex-1"
      endInset={endInset}
      items={items}
      roster={shown}
      after={after}
      tail={
        <>
          {notes?.filter((n) => n.seq === null).map(noteView)}
          <LiveRows live={thread.live} roster={shown} />
        </>
      }
      onAnswer={thread.answer}
      onOpenAttachment={openAttachment}
      onRemoveAgent={thread.removeAgent ?? undefined}
      onRetry={(text) => summon.send(text, view ?? null)}
      onReply={creating ? undefined : onReply}
      onReachTop={thread.hasMoreBefore ? thread.loadEarlier : undefined}
      empty={
        thread.status === "loading" ? null : (
          <p className="text-[13px] text-muted-foreground" data-thread-empty="">
            {thread.error ? `Couldn't read the thread: ${thread.error}` : `Nothing here yet. Write to everyone in ${spaceName}, or @mention an agent to give it a task.`}
          </p>
        )
      }
    />
  );
  const composer = (
    <>
      {thread.error && thread.items.length ? <p className="px-6 pb-1 text-xs text-muted-foreground">Reconnecting to the thread…</p> : null}
      <div className="px-5 pb-4">
        <Composer
          placeholder={!creating && addressee.handles.length ? `Message ${addressee.handles.map(nameOf).join(" and ")}` : `Message ${spaceName}`}
          candidates={candidates}
          handles={known}
          agentHarness={harnessOf}
          composerRef={composerRef}
          addressee={
            creating || !addressee.handles.length
              ? undefined
              : { handles: addressee.handles, label: nameOf, problem: problemOf, agentHandles, onRemove: (h) => unpinAgent(spaceId, h, addressee.source) }
          }
          onSend={(text) => (creating ? thread.post(text).then(() => undefined) : sendResolved(text))}
          disabled={thread.status !== "ready"}
          disabledReason={thread.role === "guest" ? GUEST_COMPOSER : null}
        />
      </div>
    </>
  );
  return (
    <ProfileScopeProvider value={scope}>
    <div data-call-layout="" className="flex h-full flex-col">
      {header}
      {notice}
      <div className="flex min-h-0 flex-1">
        {threadMode ? (
          // Thread view: the thread is the main area; the desktop shrinks
          // into a floating mini-player over it, never over the composer.
          <div data-thread-layout="" className="flex min-h-0 min-w-0 flex-1 flex-col">
            <div className="relative flex min-h-0 flex-1 flex-col">
              {threadView(reserve)}
              {streamHidden ? null : (
                <MiniPlayer spaceId={spaceId} label={stageLabel} aspect={aspect} onExpand={() => onLayout?.("call")} onReserve={setReserve}>
                  {smallStage}
                </MiniPlayer>
              )}
            </div>
            {composer}
          </div>
        ) : streamHidden ? (
          // The stream is hidden: not drawn, so its connection is released; the thread and the composer take the room.
          <div data-thread-only="" className="flex min-h-0 min-w-0 flex-1 flex-col">
            {threadView(0)}
            {composer}
          </div>
        ) : (
          <CallSplit
            leading={
              onLayout && smallStage ? (
                <Tooltip content={`Thread view (${isMacPlatform() ? "⌘⇧T" : "Ctrl+Shift+T"})`}>
                  <button
                    type="button"
                    aria-label="Thread view"
                    data-layout-toggle="thread"
                    onClick={() => onLayout("thread")}
                    className="inline-flex size-5 cursor-default items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
                  >
                    <Minimize2Icon className="size-3.5" />
                  </button>
                </Tooltip>
              ) : undefined
            }
            stage={<div className="h-full px-5 pt-4">{stage}</div>}
            below={
              <>
                {threadView(0)}
                {composer}
              </>
            }
          />
        )}
        <MembersPanel
          roster={roster}
          presence={presence}
          facts={panelFacts(facts)}
          onMore={() => setMoreOpen(true)}
          windows={windows}
          desktop={desktop}
          selected={selected}
          onSelect={
            onSelect
              ? (row) => {
                  // Picking another stream shows the stream again.
                  setStreamHidden(false);
                  onSelect(row);
                }
              : undefined
          }
          streamHidden={streamHidden}
          onToggleStream={() => setStreamHidden(!streamHidden)}
          onStream={onStream}
          folded={folded}
          onFold={setFolded}
          onAddAgent={canAdd ? () => setAddOpen(true) : undefined}
        />
      </div>
      <AddAgentSheet open={addOpen} onOpenChange={setAddOpen} space={{ id: spaceId, name: spaceName }} role={thread.role} taken={roster.agents.map((a) => a.name)}
        onShare={
          onShare
            ? () => {
                // One sheet at a time: this one closes, then Share opens in its place.
                setAddOpen(false);
                onShare();
              }
            : undefined
        }
      />
      <Dialog open={moreOpen} onOpenChange={setMoreOpen}>
        <DialogPopup data-call-more-sheet="" className="top-[8vh] flex max-h-[84vh] w-[min(820px,calc(100vw-2rem))] flex-col">
          <div className="flex h-12 shrink-0 items-center justify-between border-b pr-3 pl-5">
            <DialogTitle className="text-[13px] font-semibold">{spaceName}</DialogTitle>
            <button
              type="button"
              aria-label="Close"
              data-call-more-close=""
              onClick={() => setMoreOpen(false)}
              className="inline-flex size-7 cursor-default items-center justify-center rounded-md text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
            >
              <XIcon className="size-4" />
            </button>
          </div>
          <div className="min-h-0 overflow-y-auto px-5 pt-5 pb-2">{more}</div>
        </DialogPopup>
      </Dialog>
    </div>
    </ProfileScopeProvider>
  );
}
