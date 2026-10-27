// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { XIcon } from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";

import { Minimize2Icon } from "lucide-react";

import { Tooltip } from "@/components/ui/tooltip";
import { isMacPlatform } from "@/lib/utils";

import { harnessName, queuedNote, summonPlan, useBridge, type SpaceFact, type SpaceThread, type StreamRow, type ThreadAttachment, type ThreadView as PostView } from "@/bridge";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { toast } from "@/components/ui/toast";

import { AddAgentSheet } from "@/components/add-agent/add-agent-sheet";
import { registerAddAgent } from "@/components/add-agent/add-agent-launcher";

import { CallSplit } from "./call-split";
import { Composer } from "./composer";
import { MembersPanel, type MemberPresence } from "./members-panel";
import { panelFacts } from "./rail";
import type { SpaceLayout } from "./layout-mode";
import { MiniPlayer } from "./mini-player";
import { GUEST_COMPOSER } from "./rights";
import { buildRoster } from "./roster";
import { dismissWakeNote, OnlyYou, useSummon, useWakeNotes, WakeNoteView } from "./summon";
import { ProfileScopeProvider } from "./profile-card";
import { queuedItems, useQueued } from "./queued";
import { ThreadView } from "./thread-view";


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
  // The owner and admins change agents' rights (a daemon that does not say your role: you are its owner).
  const scope = {
    spaceId,
    roster: shown,
    shares: thread.shares,
    members: thread.members,
    role: thread.role,
    identity,
    onRemoveAgent: thread.removeAgent ?? undefined,
    onSetRights: thread.role === null || thread.role === "owner" || thread.role === "admin" ? thread.setRights : undefined,
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
      tail={notes?.filter((n) => n.seq === null).map(noteView)}
      onAnswer={thread.answer}
      onOpenAttachment={openAttachment}
      onRemoveAgent={thread.removeAgent ?? undefined}
      onRetry={(text) => summon.send(text, view ?? null)}
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
          placeholder={`Message ${spaceName}`}
          candidates={summon.candidates}
          handles={summon.handles}
          agentHarness={harnessOf}
          onSend={(text) => (creating ? thread.post(text).then(() => undefined) : summon.send(text, view ?? null))}
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
              <MiniPlayer spaceId={spaceId} label={stageLabel} aspect={aspect} onExpand={() => onLayout?.("call")} onReserve={setReserve}>
                {smallStage}
              </MiniPlayer>
            </div>
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
          onSelect={onSelect}
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
