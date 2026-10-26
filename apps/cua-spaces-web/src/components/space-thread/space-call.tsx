// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { XIcon } from "lucide-react";
import { useMemo, useState, type ReactNode } from "react";

import { harnessName, useBridge, type SpaceFact, type SpaceThread, type StreamRow, type ThreadAttachment, type ThreadView as PostView } from "@/bridge";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { toast } from "@/components/ui/toast";

import { AddAgentSheet } from "@/components/add-agent/add-agent-sheet";

import { CallSplit } from "./call-split";
import { Composer } from "./composer";
import { MembersPanel, type MemberPresence } from "./members-panel";
import { panelFacts } from "./rail";
import { GUEST_COMPOSER } from "./rights";
import { buildRoster } from "./roster";
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
}: {
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
}) {
  const { core, data } = useBridge();
  const [folded, setFolded] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  const roster = useMemo(
    () => buildRoster({ members: thread.members, shares: thread.shares, entries: thread.entries, me: thread.me, identity, role: thread.role, harnessName: (h) => harnessName(core, h) }),
    [thread.members, thread.shares, thread.entries, thread.me, thread.role, identity, core],
  );
  const harnessOf = (handle: string) => roster.agents.find((a) => a.name === handle)?.harness ?? "";
  // Attachments are Cua Volume paths: show the file where the Volume is mounted.
  const openAttachment = (a: ThreadAttachment) => {
    if (!data) return;
    data.call("volume.reveal", { path: a.path }).catch((e: unknown) =>
      toast(`Couldn't show ${a.name}`, { description: e instanceof Error ? e.message : String(e), type: "error" }),
    );
  };
  return (
    <div data-call-layout="" className="flex h-full flex-col">
      {header}
      {notice}
      <div className="flex min-h-0 flex-1">
        <CallSplit
          stage={<div className="h-full px-5 pt-4">{stage}</div>}
          below={
            <>
              <ThreadView
                className="min-h-0 flex-1"
                items={thread.items}
                roster={roster}
                onAnswer={thread.answer}
                onOpenAttachment={openAttachment}
                onRemoveAgent={thread.removeAgent ?? undefined}
                onReachTop={thread.hasMoreBefore ? thread.loadEarlier : undefined}
                empty={
                  thread.status === "loading" ? null : (
                    <p className="text-[13px] text-muted-foreground" data-thread-empty="">
                      {thread.error ? `Couldn't read the thread: ${thread.error}` : `Nothing here yet. Write to everyone in ${spaceName}, or @mention an agent to give it a task.`}
                    </p>
                  )
                }
              />
              {thread.error && thread.items.length ? <p className="px-6 pb-1 text-xs text-muted-foreground">Reconnecting to the thread…</p> : null}
              <div className="px-5 pb-4">
                <Composer
                  placeholder={`Message ${spaceName}`}
                  candidates={roster.candidates}
                  handles={roster.handles}
                  agentHarness={harnessOf}
                  onSend={(text) => thread.post(text, view ?? null)}
                  disabled={thread.status !== "ready"}
                  disabledReason={thread.role === "guest" ? GUEST_COMPOSER : null}
                />
              </div>
            </>
          }
        />
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
          // The owner and admins change agents' rights (a daemon that does not say your role: you are its owner).
          onSetRights={thread.role === null || thread.role === "owner" || thread.role === "admin" ? thread.setRights : undefined}
          onRemoveAgent={thread.removeAgent ?? undefined}
          // Anyone who can post can add an agent; a guest only reads.
          onAddAgent={thread.status === "ready" && thread.role !== "guest" ? () => setAddOpen(true) : undefined}
        />
      </div>
      <AddAgentSheet open={addOpen} onOpenChange={setAddOpen} space={{ id: spaceId, name: spaceName }} role={thread.role} taken={roster.agents.map((a) => a.name)} />
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
  );
}
