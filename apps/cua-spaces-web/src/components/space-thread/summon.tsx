// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * @mentioning agents in a Space: the composer's picker offers the
 * agents in the Space and those that can start in it; sending posts the
 * message, adds an @mentioned agent that isn't in the Space yet (asleep, so
 * the message wakes it), and, when one can't be woken, shows why under the
 * message with Connect. Once Connect finishes the message goes again by
 * itself. Which agents, what each step is and every word are the core's
 * (`summon.*`); Connect is Settings, Connectors' (`useConnectors`).
 *
 * The notes are this device's: they are about its own sign-ins, so they
 * aren't posted to the thread for everyone.
 */

import { Link } from "@tanstack/react-router";
import { CheckIcon, EyeIcon, Loader2Icon, PlugIcon, TriangleAlertIcon } from "lucide-react";
import { useCallback, useMemo, type ReactNode } from "react";
import { create } from "zustand";

import {
  summonCandidates,
  summonFailed,
  summonPlan,
  threadOpAvailable,
  useBridge,
  type CantWake,
  type DataAdapter,
  type SpaceThread,
  type SummonInput,
  type ThreadView as PostView,
} from "@/bridge";
import type { CoreClient } from "@/bridge/core";
import { Button } from "@/components/ui/button";
import { useConnectors, type Connectors } from "@/hooks/use-connectors";
import type { MentionCandidate } from "@/lib/mentions";

import type { Roster } from "./roster";

export interface WakeNote {
  id: string;
  /** The posted message it sits under (null: not posted, the host has no thread). */
  seq: number | null;
  /** The message, to send again. */
  text: string;
  cant: CantWake;
  state: "blocked" | "connecting" | "retried" | "failed";
  /** When it was noted (the Agents list's age). */
  atMs?: number;
}

/** Kept on this device only (never posted or synced), so a run that couldn't start is still listed after a reload. */
const NOTES_KEY = "cua.thread.wakeNotes";
const loadNotes = (): Record<string, WakeNote[]> => {
  try {
    const raw = globalThis.localStorage?.getItem(NOTES_KEY);
    return raw ? (JSON.parse(raw) as Record<string, WakeNote[]>) : {};
  } catch {
    return {};
  }
};
const useNotes = create<{ bySpace: Record<string, WakeNote[]> }>(() => ({ bySpace: loadNotes() }));
useNotes.subscribe((s) => {
  try {
    globalThis.localStorage?.setItem(NOTES_KEY, JSON.stringify(s.bySpace));
  } catch {
    // Not kept past this session.
  }
});

const put = (spaceId: string, note: WakeNote) =>
  useNotes.setState((s) => ({ bySpace: { ...s.bySpace, [spaceId]: [...(s.bySpace[spaceId] ?? []).filter((n) => n.id !== note.id), note] } }));
const patch = (spaceId: string, id: string, p: Partial<WakeNote>) =>
  useNotes.setState((s) => ({ bySpace: { ...s.bySpace, [spaceId]: (s.bySpace[spaceId] ?? []).map((n) => (n.id === id ? { ...n, ...p } : n)) } }));

/** Dismiss: the note goes from this device (it was never anywhere else). */
export const dismissWakeNote = (spaceId: string, id: string) =>
  useNotes.setState((s) => ({ bySpace: { ...s.bySpace, [spaceId]: (s.bySpace[spaceId] ?? []).filter((n) => n.id !== id) } }));

/** Tests start clean. */
export function resetWakeNotes(): void {
  useNotes.setState({ bySpace: {} });
}

/** Every Space's notes (the Agents list's runs that couldn't start). */
export const useAllWakeNotes = () => useNotes((s) => s.bySpace);

/** A note's move to the Space id its create became. */
export function moveWakeNotes(from: string, to: string): void {
  useNotes.setState((s) => {
    const moved = s.bySpace[from];
    if (!moved?.length) return s;
    const { [from]: _gone, ...rest } = s.bySpace;
    return { bySpace: { ...rest, [to]: [...(rest[to] ?? []), ...moved] } };
  });
}

/**
 * Connect for a note from anywhere (Home, the Agents page), then the message
 * again in its Space: the same as the note's own Connect in the thread.
 */
export async function connectNote(spaceId: string, note: WakeNote, c: { connectApp(agent: string): Promise<"connected" | "sheet" | "failed"> }, data: DataAdapter): Promise<void> {
  patch(spaceId, note.id, { state: "connecting" });
  const outcome = await c.connectApp(note.cant.app);
  if (outcome === "failed") return patch(spaceId, note.id, { state: "failed" });
  if (outcome === "sheet") return patch(spaceId, note.id, { state: "blocked" });
  await sendAgain(spaceId, note, data, async (text) => void (await data.call("thread.post", { spaceId, text })));
}

/**
 * The note's message again: its agent joins the Space first, then the
 * message is posted. When the agent can't be added, the note says why and
 * nothing is posted (a message no agent reads would look sent).
 */
async function sendAgain(spaceId: string, note: WakeNote, data: DataAdapter, post: (text: string) => Promise<void>, why?: (error: string) => CantWake | null): Promise<void> {
  try {
    await data.call("thread.agentAdd", { spaceId, name: note.cant.handle, harness: note.cant.harness });
  } catch (e) {
    const cant = why?.(e instanceof Error ? e.message : String(e));
    patch(spaceId, note.id, cant ? { state: "blocked", cant } : { state: "failed" });
    return;
  }
  patch(spaceId, note.id, { state: "retried" });
  try {
    await post(note.text);
  } catch {
    patch(spaceId, note.id, { state: "failed" });
  }
}

/** The notes under `spaceId`'s messages. */
export const useWakeNotes = (spaceId: string) => useNotes((s) => s.bySpace[spaceId]);

/**
 * Sends `text` to `spaceId`: adds each @mentioned agent the plan says to,
 * posts, and notes each one that can't be woken. Used by the composer and
 * by Ask first's first message.
 */
export async function sendWithSummon({
  core,
  data,
  spaceId,
  input,
  text,
  post,
}: {
  core: CoreClient;
  data: DataAdapter;
  spaceId: string;
  input: SummonInput;
  text: string;
  post: (text: string) => Promise<{ seq: number } | null>;
}): Promise<void> {
  const steps = summonPlan(core, input, text) ?? [];
  const blocked: CantWake[] = steps.flatMap((s) => (s.cantWake ? [s.cantWake] : []));
  for (const s of steps) {
    if (s.action !== "add") continue;
    try {
      await data.call("thread.agentAdd", { spaceId, name: s.handle, harness: s.harness });
    } catch (e) {
      const c = summonFailed(core, input, s.harness, s.handle, e instanceof Error ? e.message : String(e));
      if (c) blocked.push(c);
    }
  }
  const entry = await post(text);
  for (const c of blocked) {
    put(spaceId, { id: `${entry?.seq ?? Date.now()}:${c.handle}`, seq: entry?.seq ?? null, text, cant: c, state: "blocked", atMs: Date.now() });
  }
}

/** The Space's summon: the picker's candidates, the handles that draw as chips, and send. */
export function useSummon({ spaceId, thread, roster, canAdd }: { spaceId: string; thread: SpaceThread; roster: Roster; canAdd: boolean }) {
  const { core, data } = useBridge();
  const connectors = useConnectors();
  const input: SummonInput = useMemo(
    () => ({
      members: thread.members.map((m) => ({ handle: m.handle ?? m.name, harness: m.harness, label: roster.agents.find((a) => a.name === m.name)?.label ?? m.name })),
      people: roster.people.map((p) => p.label.toLowerCase().replace(/\s+/g, "")),
      canAdd: canAdd && !!data && threadOpAvailable(data.mode, "thread.agentAdd"),
      connectors: connectors.input,
    }),
    [thread.members, roster, canAdd, data, connectors.input],
  );
  const extra = useMemo<MentionCandidate[]>(
    () =>
      (summonCandidates(core, input) ?? [])
        .filter((c) => !c.member)
        .map((c) => ({ handle: c.handle, label: c.label, kind: "agent" as const, harness: c.harness, detail: c.detail })),
    [core, input],
  );
  const candidates = useMemo(() => {
    const agents = roster.candidates.filter((c) => c.kind === "agent");
    const people = roster.candidates.filter((c) => c.kind !== "agent");
    return [...agents, ...extra, ...people];
  }, [roster.candidates, extra]);
  const handles = useMemo(() => new Set([...roster.handles, ...extra.map((c) => c.handle.toLowerCase())]), [roster.handles, extra]);

  const send = useCallback(
    async (text: string, view: PostView | null, replyTo?: number | null) => {
      if (!data) return;
      await sendWithSummon({ core, data, spaceId, input, text, post: (t) => thread.post(t, view, replyTo) });
    },
    [core, data, spaceId, input, thread],
  );

  /** Connect for a note, then the message again. */
  const connectAndRetry = useCallback(
    async (note: WakeNote, c: Connectors) => {
      patch(spaceId, note.id, { state: "connecting" });
      const outcome = await c.connectApp(note.cant.app);
      if (outcome === "failed") return patch(spaceId, note.id, { state: "failed" });
      // A provider sheet asks for a key first: the note waits; Send again is the next step.
      if (outcome === "sheet") return patch(spaceId, note.id, { state: "blocked" });
      await retry(note);
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [spaceId],
  );

  const retry = async (note: WakeNote) => {
    if (!data) return;
    await sendAgain(
      spaceId,
      note,
      data,
      async (text) => void (await thread.post(text, null)),
      (error) => summonFailed(core, input, note.cant.harness, note.cant.handle, error),
    );
  };

  return { candidates, handles, send, connectors, connectAndRetry, retry };
}

/** One "can't be woken" line under its message: why, Connect, the app's page; then "Sent again". */
export function WakeNoteView({
  note,
  connected,
  onConnect,
  onRetry,
  onDismiss,
}: {
  note: WakeNote;
  onDismiss: () => void;
  /** The app has a sign-in now (connected elsewhere, or a key just pasted). */
  connected: boolean;
  onConnect: () => void;
  onRetry: () => void;
}) {
  const c = note.cant;
  let body: ReactNode;
  if (note.state === "retried") {
    body = (
      <p className="flex items-center gap-1.5 text-[12.5px] text-muted-foreground" data-wake-note-retried="">
        <CheckIcon className="size-3.5 text-success" /> {c.retriedText}
      </p>
    );
  } else {
    body = (
      <>
        <p className="flex items-center gap-1.5 text-[13px] font-medium">
          <TriangleAlertIcon className="size-3.5 shrink-0 text-warning" /> {c.text}
        </p>
        <p className="mt-0.5 text-[12.5px] text-muted-foreground">{c.detail}</p>
        <div className="mt-2 flex flex-wrap items-center gap-2">
          {connected && note.state !== "connecting" ? (
            <Button size="sm" variant="outline" onClick={onRetry} data-wake-retry="">
              {c.retryLabel}
            </Button>
          ) : c.connectLabel ? (
            <Button size="sm" variant="outline" disabled={note.state === "connecting"} onClick={onConnect} data-wake-connect="">
              {note.state === "connecting" ? <Loader2Icon className="animate-spin" /> : <PlugIcon />}
              {note.state === "connecting" ? c.connectingLabel : c.connectLabel}
            </Button>
          ) : null}
          {c.openLabel ? (
            <Button size="sm" variant="ghost" className="text-muted-foreground" render={<Link to="/settings/connections/$app" params={{ app: c.app }} />} data-wake-open="">
              {c.openLabel}
            </Button>
          ) : null}
        </div>
      </>
    );
  }
  return (
    <div role="status" data-wake-note={c.handle} data-state={note.state} data-only-you={c.onlyYou ? "" : undefined} className="my-1.5 ml-11 rounded-lg bg-brand-surface px-3.5 py-2.5">
      <OnlyYou label={c.onlyYouLabel} dismiss={note.state === "connecting" ? null : c.dismissLabel} onDismiss={onDismiss} />
      {body}
    </div>
  );
}

/** "Only visible to you" with its eye, and Dismiss where it applies: on every line only this device shows. */
export function OnlyYou({ label, dismiss, onDismiss }: { label: string; dismiss?: string | null; onDismiss?: () => void }) {
  return (
    <div className="mb-1 flex items-center gap-1.5 text-[11px] text-muted-foreground" data-only-you-label="">
      <EyeIcon aria-hidden className="size-3" />
      <span className="flex-1">{label}</span>
      {dismiss && onDismiss ? (
        <button type="button" data-wake-dismiss="" onClick={onDismiss} className="cursor-default rounded-lg px-1 outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60">
          {dismiss}
        </button>
      ) : null}
    </div>
  );
}
