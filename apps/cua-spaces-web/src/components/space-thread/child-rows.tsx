// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Child agents in the thread: the subagent an agent started (what it was
 * asked, whether it runs, is done or failed, how many steps, what it handed
 * back) and the child Space it made (a link to it). They sit under the
 * agent's messages, indented to its text, as quiet rows: a mark for the
 * state, the task in a line, the facts in a second muted line, the result
 * behind a disclosure. The core writes the notes (`event.kind` `subagent`
 * and `child_space`); a subagent's notes fold into one row (`foldThread`).
 */

import { Link } from "@tanstack/react-router";
import { CheckIcon, XIcon } from "lucide-react";

import type { LiveChild, SpaceOs, ThreadItem } from "@/bridge";
import { OsIcon } from "@/components/os-icon";

import { WorkingFor, elapsedText } from "./live-row";

const stepsText = (n: number) => (n === 1 ? "1 step" : `${n} steps`);

/** The state's mark: a pulsing dot while it runs, a check when done, a cross when it failed. */
export function ChildMark({ state }: { state: "running" | "done" | "failed" }) {
  if (state === "done") return <CheckIcon aria-hidden className="size-3.5 text-muted-foreground" />;
  if (state === "failed") return <XIcon aria-hidden className="size-3.5 text-destructive" />;
  return <span aria-hidden className="live-dot size-1.5 rounded-full bg-brand" />;
}

const STATE_WORD = { running: "Running", done: "Done", failed: "Failed" } as const;

/** One subagent, from its folded notes. */
export function SubagentRow({ item }: { item: Extract<ThreadItem, { type: "subagent" }> }) {
  const { sub } = item;
  const took = sub.startedMs != null && sub.endedMs != null ? elapsedText(sub.endedMs - sub.startedMs) : null;
  const facts = (
    <span className="flex flex-wrap items-center gap-x-3 text-[12px] text-muted-foreground">
      {sub.state === "running" && sub.startedMs != null ? (
        <span data-subagent-state={sub.state}>
          <WorkingFor label="Running" sinceMs={sub.startedMs} />
        </span>
      ) : (
        <span data-subagent-state={sub.state}>{STATE_WORD[sub.state]}</span>
      )}
      {sub.steps > 0 ? <span>{stepsText(sub.steps)}</span> : null}
      {took ? <span className="tabular-nums">{took}</span> : null}
    </span>
  );
  const head = (
    <span className="flex min-w-0 items-start gap-2.5">
      <span className="mt-[5px] flex size-3.5 shrink-0 items-center justify-center">
        <ChildMark state={sub.state} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[13px] text-foreground">{sub.title}</span>
        {facts}
      </span>
    </span>
  );
  return (
    <div data-subagent={sub.id} data-subagent-state={sub.state} className="ml-10 py-1">
      {sub.result ? (
        <details className="group">
          <summary className="cursor-pointer list-none">{head}</summary>
          <p className="mt-1 ml-6 text-[12px] break-words whitespace-pre-wrap text-muted-foreground">{sub.result}</p>
        </details>
      ) : (
        head
      )}
    </div>
  );
}

const OS_KEY = (os: string) => (os === "macOS" ? "macos" : os === "Windows" ? "windows" : os === "Linux" ? "linux" : "unknown") as SpaceOs;

/**
 * A child Space the agent made, drawn as a subagent row: the same quiet mark, a line for the
 * Space (its OS logo and name), a muted line for where it is ("Cloning the disk", "Ready in 1m 04s",
 * "Failed: ..."). The whole row opens the child's page, which exists from the moment the create starts.
 */
/** The page a child row opens: the Space once it exists, else its being-made row (the same page a New Space has). */
export function childPageId(space: { space: string; key?: string; state?: string }): string {
  if (space.state === "ready" || !space.key) return space.space;
  return `pending:agent-${space.key.replace(/:(\d+)$/, "-$1")}`;
}

export function ChildSpaceRow({ item }: { item: Extract<ThreadItem, { type: "child_space" }> }) {
  const { space } = item;
  const state = space.state ?? "ready";
  const mark = state === "creating" ? "running" : state === "failed" ? "failed" : "done";
  const took = space.startedMs != null && space.endedMs != null ? elapsedText(space.endedMs - space.startedMs) : null;
  const facts =
    state === "creating" ? (
      <span className="flex flex-wrap items-center gap-x-3 text-[12px] text-muted-foreground">
        <span data-child-space-phase>
          {space.phase || "Starting"}
          {space.percent != null ? `, ${space.percent}%` : ""}
        </span>
        {space.startedMs != null ? <WorkingFor label="" sinceMs={space.startedMs} /> : null}
      </span>
    ) : state === "failed" ? (
      <span data-child-space-phase className="block text-[12px] text-muted-foreground">
        Couldn't make it: {space.error || "something went wrong"}
      </span>
    ) : (
      <span data-child-space-phase className="block text-[12px] text-muted-foreground">
        Ready{took ? ` in ${took}` : ""}
      </span>
    );
  return (
    <div data-child-space={space.space} data-child-space-state={state} className="ml-10 py-1">
      <Link
        to="/spaces/$spaceId"
        params={{ spaceId: childPageId(space) }}
        className="flex min-w-0 items-start gap-2.5 rounded-lg hover:bg-muted/40"
        aria-label={`Open ${space.name}`}
      >
        <span className="mt-[5px] flex size-3.5 shrink-0 items-center justify-center">
          <ChildMark state={mark} />
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex min-w-0 items-center gap-1.5 text-[13px] text-foreground">
            <OsIcon os={OS_KEY(space.os)} className="size-3.5 shrink-0 text-muted-foreground" />
            <span className="truncate">{space.name}</span>
          </span>
          {facts}
        </span>
      </Link>
    </div>
  );
}

/** The subagents an agent has running now, under its working line. */
export function LiveChildren({ children }: { children: readonly LiveChild[] }) {
  if (!children.length) return null;
  return (
    <ul data-live-children="" className="mt-1 ml-10 space-y-0.5">
      {children.map((c) => (
        <li key={c.id} data-live-child={c.id} className="flex min-w-0 items-start gap-2.5">
          <span className="mt-[5px] flex size-3.5 shrink-0 items-center justify-center">
            <ChildMark state="running" />
          </span>
          <span className="min-w-0 flex-1">
            <span className="block truncate text-[13px]">{c.title}</span>
            <span className="block truncate text-[12px] text-muted-foreground">
              {c.step ?? "Starting"}
              {c.steps > 0 ? `, ${stepsText(c.steps)}` : ""}
              {c.sinceMs ? (
                <>
                  {", "}
                  <WorkingFor label="Running" sinceMs={c.sinceMs} />
                </>
              ) : null}
            </span>
          </span>
        </li>
      ))}
    </ul>
  );
}
