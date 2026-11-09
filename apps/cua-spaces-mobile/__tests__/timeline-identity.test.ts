// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { foldThread } from "../src/data/thread/fold";
import { buildRows } from "../src/features/thread/timeline";
import { entry, ME, SAM } from "../src/data/demo/data";

const agent = (seq: number, name: string, text: string) => entry(seq, 1_000 + seq, { kind: "agent", id: `agent:${name}`, name } as never, text, {});

describe("who an item is shown as", () => {
  const items = foldThread([entry(1, 1_000, SAM, "hi"), agent(2, "claude", "On it."), agent(3, "claude-2", "Me too."), agent(4, "ghost", "Who am I?")], { me: ME.id });
  const rows = (members?: string[]) => buildRows({ items, me: ME.id, live: [], pending: [], tools: [], now: 2_000, agentName: (h) => h, members: members ? new Set(members) : undefined });
  const authors = (r: ReturnType<typeof rows>) => r.filter((x) => x.type === "message").map((x) => (x as { entry: { author: { kind: string; name: string } } }).entry.author);

  it("a member is itself, a person is the person, an unmatched agent is Cua's system author", () => {
    const a = authors(rows(["claude", "claude-2"]));
    expect(a.map((x) => [x.kind, x.name])).toEqual([["person", SAM.name], ["agent", "claude"], ["agent", "claude-2"], ["system", "Cua"]]);
  });

  it("without a member list nothing is replaced", () => {
    expect(authors(rows()).map((x) => x.name)).toEqual([SAM.name, "claude", "claude-2", "ghost"]);
  });
});
