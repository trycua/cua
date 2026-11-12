// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo's "Pick up where you left off" (CUA-3319): three made-up chats,
 * one per agent, as the daemon's `agent_threads` / `agent_thread_read`
 * answer them. `?demo=nochats` plays a computer with none.
 */

import { HostError } from "../../adapter";
import { onlyAgents, type PastThread, type RecentThread } from "../../ops/agent-threads";

const MIN = 60_000;

export function demoAgentThreads(now: () => number, none: boolean) {
  const threads: PastThread[] = none
    ? []
    : [
        {
          thread: {
            agent: "claude-code",
            id: "0b1e5c2a-1111-4222-8333-944445555666",
            title: "Kettle timer test fails on CI",
            cwd: "/Users/demo/code/kettle",
            lastActiveMs: now() - 25 * MIN,
            resume: ["claude", "--resume", "0b1e5c2a-1111-4222-8333-944445555666"],
          },
          messages: [
            {
              key: "m1",
              role: "person",
              text: "The kettle timer test fails on CI but passes locally, can you find out why?",
              tsMs: now() - 40 * MIN,
              cut: false,
            },
            {
              key: "m2",
              role: "agent",
              text: "It depends on the machine's time zone: CI runs in UTC and the test expects local time.",
              tsMs: now() - 38 * MIN,
              cut: false,
            },
            { key: "m3", role: "person", text: "Makes sense. Let's look again tomorrow.", tsMs: now() - 25 * MIN, cut: false },
          ],
          earlierOmitted: 0,
        },
        {
          thread: {
            agent: "openai-codex",
            id: "5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5",
            title: "Checkout double charge",
            cwd: "/Users/demo/code/bike-shop",
            lastActiveMs: now() - 3 * 60 * MIN,
            resume: ["codex", "resume", "5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5"],
          },
          messages: [
            { key: "u-1", role: "person", text: "Why does checkout double-charge when the coupon is applied twice?", tsMs: now() - 3 * 60 * MIN, cut: false },
            {
              key: "a-1",
              role: "agent",
              text: "The coupon handler isn't idempotent. I'd key it on the order id.",
              tsMs: now() - 3 * 60 * MIN + MIN,
              cut: false,
            },
          ],
          earlierOmitted: 0,
        },
        {
          thread: {
            agent: "hermes",
            id: "20261103_201500_ab12cd",
            title: "Summarise my notes on the bakery lease",
            cwd: "/Users/demo/notes",
            lastActiveMs: now() - 26 * 60 * MIN,
            resume: ["hermes", "--resume", "20261103_201500_ab12cd"],
          },
          messages: [
            { key: "1", role: "person", text: "Summarise my notes on the bakery lease", tsMs: now() - 26 * 60 * MIN, cut: false },
            {
              key: "4",
              role: "agent",
              text: "The lease renews in March; the rent review clause is in section 4.",
              tsMs: now() - 26 * 60 * MIN + MIN,
              cut: false,
            },
          ],
          earlierOmitted: 0,
        },
      ];
  const copy = (t: RecentThread): RecentThread => ({ ...t, resume: [...t.resume] });
  return {
    "agents.recentThreads": ({ limit, agents }: { limit?: number; agents?: string[] }) =>
      onlyAgents(
        threads.map((t) => t.thread),
        agents,
      )
        .slice(0, limit ?? 4)
        .map(copy),
    "agents.threadHistory": ({ agent, id }: { agent: string; id: string }) => {
      const t = threads.find((x) => x.thread.agent === agent && x.thread.id === id);
      if (!t) throw new HostError(`no ${agent} chat ${id} on this computer`, "not_found");
      return { thread: copy(t.thread), messages: t.messages.map((m) => ({ ...m })), earlierOmitted: t.earlierOmitted };
    },
  };
}
