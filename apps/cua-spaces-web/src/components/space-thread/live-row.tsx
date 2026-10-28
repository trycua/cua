// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The live row at the bottom of the thread for each agent that is at work
 * (CUA-2640): "hermes-test is starting...", then the stage the run reports
 * (installing, starting, thinking, using Bash). The daemon sends the stages
 * with the thread's own follow, so a row shows as the stage changes; it is
 * gone when the agent replies or fails (the failure is its own message).
 */

import type { LiveAgent } from "@/bridge";

import { MemberAvatar } from "./avatar";
import type { Roster } from "./roster";

/** `thinking` as "is thinking"; `using Bash` as "is using Bash". */
export const stageSentence = (stage: string) => `is ${stage}`;

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

export function LiveRows({ live, roster }: { live: readonly LiveAgent[]; roster: Roster }) {
  if (!live.length) return null;
  return (
    <>
      {live.map((a) => {
        const member = roster.agents.find((m) => m.handle === a.agent || m.name === a.agent);
        const label = member?.label ?? a.agent;
        return (
          <div key={a.agent} data-live-row={a.agent} data-live-stage={a.stage} role="status" className="flex items-center gap-3 py-2">
            <MemberAvatar kind="agent" name={member?.handle ?? a.agent} harness={member?.harness ?? ""} identity={member?.identity} size={28} />
            <p className="min-w-0 truncate text-[13px] text-muted-foreground">
              <span className="font-medium text-foreground">{label}</span> {stageSentence(a.stage)}
              <Dots />
            </p>
          </div>
        );
      })}
    </>
  );
}
