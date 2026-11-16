// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { mentionAt, rankMentions, type Mentionable } from "../src/features/thread/Composer";
import { buildRows } from "../src/features/thread/timeline";
import { toolGroup } from "../src/core/phone";
import { foldThread } from "../src/data/thread/fold";
import { entry, ME, CLAUDE, CHECKOUT_TOOL_ROWS_WIRE } from "../src/data/demo/data";
import { toolRowsFromTool } from "../src/data/thread/phone-protocol";
import { installFixtureCore, uninstallCore } from "./helpers/core";

beforeAll(installFixtureCore);
afterAll(uninstallCore);

const CHECKOUT_GROUPS = toolRowsFromTool(CHECKOUT_TOOL_ROWS_WIRE).groups;

const all: Mentionable[] = [
  { handle: "claude", name: "Claude Code", kind: "agent", note: null },
  { handle: "reviewer", name: "Reviewer", kind: "agent", note: "asleep" },
  { handle: "sam", name: "Sam", kind: "person", note: null },
];

describe("the mention picker", () => {
  it("opens for an @ at the start or after a space, with what was typed", () => {
    expect(mentionAt("@cl", 3)).toEqual({ start: 0, query: "cl" });
    expect(mentionAt("hi @", 4)).toEqual({ start: 3, query: "" });
    expect(mentionAt("mail me@x", 9)).toBeNull();
    expect(mentionAt("@claude done", 12)).toBeNull();
  });
  it("lists agents before people and filters by the start of the handle or name", () => {
    expect(rankMentions(all, "").map((m) => m.handle)).toEqual(["claude", "reviewer", "sam"]);
    expect(rankMentions(all, "re").map((m) => m.handle)).toEqual(["reviewer"]);
    expect(rankMentions(all, "sa").map((m) => m.handle)).toEqual(["sam"]);
  });
});

describe("tool calls read as one muted phrase for the group", () => {
  it("uses the app core's words (the table of cua-agents' tool_row, checked in Rust)", () => {
    expect(toolGroup(CHECKOUT_GROUPS[0]!.calls)?.phrase).toBe("Read 2 files, edited 2 files, ran 1 command");
    const act = { cat: "computer", line: "Clicked" };
    expect(toolGroup([act, act, act])?.phrase).toBe("Used 3 machine actions");
    expect(toolGroup([{ cat: "searched", line: "Searched" }])?.phrase).toBe("Searched once");
  });
});

describe("the timeline", () => {
  const now = 1_800_000_000_000;
  const items = foldThread([entry(1, now - 90 * 60_000, CLAUDE, "a"), entry(2, now - 89 * 60_000, CLAUDE, "b"), entry(3, now - 60_000, ME, "c")], {
    me: ME.id,
  });
  it("shows the author once for a run of messages and starts a day line after a gap", () => {
    const rows = buildRows({ items, me: ME.id, live: [], pending: [], tools: [{ ...CHECKOUT_GROUPS[0]!, afterSeq: 2 }], now, agentName: (h) => h });
    expect(rows.map((r) => r.type)).toEqual(["day", "message", "message", "tools", "day", "message"]);
    expect(rows.filter((r) => r.type === "message").map((r) => (r as { showAuthor: boolean }).showAuthor)).toEqual([true, false, false]);
  });
  it("adds the live agent's words and the messages the machine has not taken at the end, and an answer shows on its card instead", () => {
    const pending = (id: string, kind: string, state: "waiting" | "sending" | "failed") => ({
      id,
      space: "s",
      kind,
      text: "oops",
      state,
      label: "Not sent",
      canRetry: state === "failed",
      canDiscard: true,
      retryLabel: "Retry",
      discardLabel: "Delete",
      mode: "send",
    });
    const rows = buildRows({
      items,
      me: ME.id,
      live: [{ agent: "@claude", stage: "replying", text: "On it" }],
      pending: [pending("f1", "post", "failed"), pending("a1", "answer", "waiting")],
      tools: [],
      now,
      agentName: () => "Claude Code",
    });
    expect(rows.slice(-2).map((r) => r.type)).toEqual(["live", "pending"]);
    expect(rows.filter((r) => r.type === "pending")).toHaveLength(1);
  });
  it("puts the calls of every agent that followed one message in one group, in the order they happened", () => {
    const g = (agent: string, at: number) => ({
      afterSeq: 2,
      agent,
      calls: [{ id: `${agent}${at}`, cat: "ran", line: `Ran ${agent}`, detail: "", state: "done" as const, atMs: at }],
    });
    const rows = buildRows({ items, me: ME.id, live: [], pending: [], tools: [g("b", 20), g("a", 10)], now, agentName: (h) => h });
    const tools = rows.find((r) => r.type === "tools") as { calls: { line: string }[] };
    expect(tools.calls.map((c) => c.line)).toEqual(["Ran a", "Ran b"]);
  });
});

import { parseBlocks, parseInline } from "../src/features/thread/markdown";

describe("markdown in an agent's message (safe while it streams)", () => {
  it("reads bold, code, https links and mentions, and ignores other links", () => {
    expect(parseInline("**done** run `npm test` see [log](https://x.io/a) @sam [bad](javascript:alert(1))")).toEqual([
      { t: "bold", v: "done" },
      { t: "text", v: " run " },
      { t: "code", v: "npm test" },
      { t: "text", v: " see " },
      { t: "link", v: "log", href: "https://x.io/a" },
      { t: "text", v: " " },
      { t: "mention", v: "@sam" },
      { t: "text", v: " [bad](javascript:alert(1))" },
    ]);
  });
  it("draws an unclosed fence as code so far and an unclosed ** as plain text", () => {
    expect(parseBlocks("Result:\n\n```sh\nnpm test\n149 pa")).toEqual([
      { t: "p", inline: [{ t: "text", v: "Result:" }] },
      { t: "code", v: "npm test\n149 pa" },
    ]);
    expect(parseInline("a **bol")).toEqual([{ t: "text", v: "a **bol" }]);
  });
});
