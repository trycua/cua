// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * What an agent that is at work shows at the bottom of the thread
 * (CUA-2640): "hermes-test is starting...", then the stage the run reports
 * (installing, starting, thinking, using Bash), and, once it has begun to
 * answer, what it has said so far as one live entry that the next snapshot
 * replaces whole (never appended to). Under it, "Working for 1m 05s": the
 * clock ticks by writing its own text node, so a second passing re-renders
 * nothing. The daemon sends all of it with the thread's own follow. The row
 * is gone when the agent replies or fails (the reply, or the failure, is
 * then its own message).
 */

import { useEffect, useRef } from "react";

import type { LiveAgent } from "@/bridge";

import { MemberAvatar } from "./avatar";
import type { Roster } from "./roster";
import { ThreadText } from "./thread-view";

/** `thinking` as "is thinking"; `using Bash` as "is using Bash". */
export const stageSentence = (stage: string) => `is ${stage}`;

/** "12s", "1m 05s", "1h 02m": how long the agent has been at work. */
export function elapsedText(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}

/** Three dots that pulse in turn (still, when the person asked for less motion). */
function Dots() {
  return (
    <span aria-hidden data-live-dots="" className="inline-flex">
      <span className="live-dot">.</span>
      <span className="live-dot">.</span>
      <span className="live-dot">.</span>
    </span>
  );
}

/** "Working for 1m 05s", ticking once a second by writing its text directly (no render per tick). */
export function WorkingFor({ sinceMs, now = Date.now }: { sinceMs: number | undefined; now?: () => number }) {
  const ref = useRef<HTMLSpanElement | null>(null);
  // Without the daemon's clock, from when this row first showed.
  const from = useRef(sinceMs ?? now());
  useEffect(() => {
    const start = sinceMs ?? from.current;
    const write = () => {
      if (ref.current) ref.current.textContent = `Working for ${elapsedText(now() - start)}`;
    };
    write();
    const t = setInterval(write, 1000);
    return () => clearInterval(t);
  }, [sinceMs, now]);
  return <span ref={ref} data-live-elapsed="" className="tabular-nums" />;
}

export function LiveRows({ live, roster }: { live: readonly LiveAgent[]; roster: Roster }) {
  if (!live.length) return null;
  return (
    <>
      {live.map((a) => {
        const member = roster.agents.find((m) => m.handle === a.agent || m.name === a.agent);
        const label = member?.label ?? a.agent;
        const avatar = <MemberAvatar kind="agent" name={member?.handle ?? a.agent} harness={member?.harness ?? ""} identity={member?.identity} size={28} />;
        return (
          <div key={a.agent} data-live-row={a.agent} data-live-stage={a.stage} role="status" className="py-2">
            {a.text ? (
              // What it has said so far: one entry, replaced whole by each snapshot.
              <div data-live-entry={a.agent} className="mb-1 flex gap-3">
                <span className="mt-0.5 shrink-0">{avatar}</span>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate text-[13px] font-semibold">{label}</span>
                  </div>
                  <ThreadText text={a.text} handles={roster.handles} />
                </div>
              </div>
            ) : null}
            <div className="flex items-center gap-3">
              {a.text ? <span className="size-7 shrink-0" aria-hidden /> : <span className="shrink-0">{avatar}</span>}
              <p className="min-w-0 truncate text-[13px] text-muted-foreground">
                <span className="font-medium text-foreground">{label}</span> {stageSentence(a.stage)}
                <Dots /> <WorkingFor sinceMs={a.sinceMs} />
              </p>
            </div>
          </div>
        );
      })}
    </>
  );
}
