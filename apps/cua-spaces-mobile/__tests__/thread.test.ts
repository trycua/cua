// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { foldThread, readSeqFrom } from "../src/data/thread/fold";
import { threadEntryFromTool, threadPageFromTool, threadReadArgs, threadPostArgs } from "../src/data/thread/protocol";
import { entry, ME, SAM, CLAUDE, CUA } from "../src/data/demo/data";

describe("the thread wire (same mapping as the web app's bridge/ops/thread.ts)", () => {
  it("maps tool JSON (snake_case) to an entry, and reads a card only from Cua's own host", () => {
    const base = {
      seq: 7,
      id: "e7",
      ts_ms: 5,
      text: "wants to push",
      kind: "approval",
      approval: { action: "export:repo.push", details: "x", state: "open", card: { repo: "acme/app" } },
    };
    const fromHost = threadEntryFromTool({ ...base, author: { kind: "system", id: "system:host", name: "Cua" } });
    expect(fromHost?.approval?.card).toEqual({ repo: "acme/app" });
    const fromAgent = threadEntryFromTool({ ...base, author: { kind: "agent", id: "agent:claude", name: "claude" } });
    expect(fromAgent?.approval?.card).toBeNull();
  });

  it("maps a page with its cursor, live agents and role", () => {
    const p = threadPageFromTool({
      entries: [],
      cursor: 12,
      caught_up: true,
      unread: 2,
      role: "owner",
      me: { id: "acct:you" },
      live: { key: "k1", agents: [{ agent: "@claude", stage: "using Bash", since_ms: 9, text: "Running" }] },
    });
    expect(p.cursor).toBe(12);
    expect(p.role).toBe("owner");
    expect(p.me).toBe("acct:you");
    expect(p.live.agents[0]).toEqual({ agent: "@claude", stage: "using Bash", sinceMs: 9, text: "Running" });
  });

  it("builds the tool arguments the daemon takes", () => {
    expect(threadReadArgs({ spaceId: "relay:a", after: 3, limit: 100, markRead: true, followMs: 99999, liveAfter: "k" })).toEqual({
      space: "relay:a",
      after: 3,
      limit: 100,
      mark_read: true,
      follow_ms: 25000,
      live_after: "k",
    });
    expect(threadPostArgs({ spaceId: "relay:a", text: "Approved", answer: { approval: 7, approve: true } })).toEqual({
      space: "relay:a",
      text: "Approved",
      answer: { approval: 7, approve: true },
    });
  });
});

describe("foldThread", () => {
  const t0 = 1_000_000;
  const entries = [
    entry(1, t0, CLAUDE, "Morning."),
    entry(2, t0 + 1, SAM, "Hi @claude", { task: { to: "claude", session: null }, mentions: ["claude"] }),
    entry(3, t0 + 2, CUA, "approve?", { kind: "approval", approval: { action: "a", details: "d", state: "open", card: null } }),
  ];

  it("keeps an approval open until an answer names it, then folds the answer into it", () => {
    const open = foldThread(entries, { me: ME.id });
    expect(open.find((i) => i.type === "approval")).toMatchObject({ answer: null });
    const answered = foldThread([...entries, entry(4, t0 + 3, ME, "Approved", { kind: "answer", answer: { approval: 3, approve: true, note: null } })], {
      me: ME.id,
    });
    expect(answered.find((i) => i.type === "approval")).toMatchObject({ answer: { approve: true } });
    expect(answered.some((i) => i.type === "message" && i.entry.seq === 4)).toBe(false);
  });

  it("puts the since-you-were-here line before the first entry someone else wrote after the reader's cursor", () => {
    const items = foldThread(entries, { readSeq: 1, me: ME.id });
    const at = items.findIndex((i) => i.type === "unread");
    expect(at).toBe(1);
    expect((items[at + 1] as { entry: { seq: number } }).entry.seq).toBe(2);
  });

  it("derives the cursor from the unread count when the daemon does not say it", () => {
    expect(readSeqFrom(entries, 1, ME.id)).toBe(2);
    expect(readSeqFrom(entries, 0, ME.id)).toBe(3);
  });
});

describe("every agent message and the child agents", () => {
  const t0 = 2_000_000;
  const task = entry(1, t0, SAM, "@claude do the thing", { task: { to: "claude", session: null }, mentions: ["claude"] });
  const said = (seq: number, text: string) => entry(seq, t0 + seq, CLAUDE, text, { replyTo: 1 });
  const note = (seq: number, child: unknown) => entry(seq, t0 + seq, CUA, "note", { kind: "system", child } as never);

  it("keeps each message in order, including one after the task closed", () => {
    const items = foldThread([task, said(2, "On it."), said(3, "Found them."), said(4, "Late news.")], { me: ME.id });
    expect(items.filter((i) => i.type === "message").map((i) => (i as { entry: { text: string } }).entry.text)).toEqual([
      "On it.",
      "Found them.",
      "Late news.",
    ]);
  });

  it("folds a subagent's notes into one row with its latest state, and draws a child Space", () => {
    const sub = (seq: number, state: string) =>
      note(seq, { kind: "subagent", id: "t1", agent: "claude", title: "Map", state, steps: 2, result: null, startedMs: null, endedMs: null });
    const items = foldThread(
      [
        task,
        said(2, "Started."),
        sub(3, "running"),
        sub(4, "done"),
        note(5, { kind: "child_space", space: "local:s", name: "s", os: "Linux", agent: "claude" }),
      ],
      { me: ME.id },
    );
    expect(items.map((i) => i.type)).toEqual(["task", "message", "subagent", "child_space"]);
    expect((items[2] as { sub: { state: string } }).sub.state).toBe("done");
  });

  it("folds the notes of one Space create into one row with its latest state and reads the daemon's JSON", () => {
    const k = (seq: number, state: string, extra: object = {}) =>
      note(seq, { kind: "child_space", space: "local:mac", key: "mac:1", name: "mac", os: "macOS", agent: "claude", state, ...extra });
    const items = foldThread([task, k(2, "creating", { phase: "Starting it" }), k(3, "failed", { error: "Its network setup did not come up" })], { me: ME.id });
    const rows = items.filter((i) => i.type === "child_space");
    expect(rows).toHaveLength(1);
    expect((rows[0] as { space: { state: string; error: string } }).space).toMatchObject({ state: "failed", error: "Its network setup did not come up" });
    const e = threadPageFromTool({
      entries: [
        {
          seq: 9,
          id: "x",
          ts_ms: 1,
          author: { kind: "system", id: "system:dispatcher", name: "Cua" },
          kind: "system",
          text: "t",
          event: {
            kind: "child_space",
            space: "local:mac",
            key: "mac:1",
            name: "mac",
            os: "macOS",
            agent: "claude",
            state: "creating",
            phase: "Starting it",
            percent: 40,
            started_ms: 5,
          },
        },
      ],
      cursor: 9,
      caught_up: true,
      unread: 0,
      role: "owner",
    }).entries[0]!;
    expect(e.child).toMatchObject({ kind: "child_space", state: "creating", phase: "Starting it", percent: 40, startedMs: 5, key: "mac:1" });
  });

  describe("a created Space's row sits under its creator", () => {
    const kid = (seq: number, key: string, extra: object = {}) =>
      note(seq, { kind: "child_space", space: `local:${key}`, key: `${key}:1`, name: key, os: "Linux", agent: "claude", state: "creating", ...extra });
    const shape = (items: ReturnType<typeof foldThread>) =>
      items.map((i) => (i.type === "message" ? `${i.entry.author.name}${i.entry.text ? ":" + i.entry.text : ":header"}` : i.type));

    it("follows the agent's own text with no second header", () => {
      expect(shape(foldThread([task, said(2, "On it."), kid(3, "a")], { me: ME.id }))).toEqual(["task", "Claude Code:On it.", "child_space"]);
    });

    it("brings the agent's header with the row when it has not spoken yet, and groups several creates under it", () => {
      const items = foldThread([task, kid(2, "a"), kid(3, "b"), kid(4, "a", { state: "ready" })], { me: ME.id });
      expect(shape(items)).toEqual(["task", "claude:header", "child_space", "child_space"]);
      expect(items.filter((i) => i.type === "child_space").map((i) => (i as { space: { name: string; state: string } }).space.state)).toEqual([
        "ready",
        "creating",
      ]);
    });

    it("sits under the person's header when a person made it, and alone when nobody is recorded", () => {
      expect(shape(foldThread([kid(2, "p", { agent: "", by: { id: "acct_ada", name: "Ada" } })], { me: ME.id }))).toEqual(["Ada:header", "child_space"]);
      expect(shape(foldThread([kid(2, "z", { agent: "" })], { me: ME.id }))).toEqual(["child_space"]);
    });
  });

  it("reads the live children from the daemon", () => {
    const p = threadPageFromTool({
      entries: [],
      cursor: 1,
      caught_up: true,
      unread: 0,
      role: "owner",
      live: { key: "k", agents: [{ agent: "@claude", stage: "thinking", children: [{ id: "t1", title: "Map", steps: 1, step: "Ran ls" }] }] },
    });
    expect(p.live.agents[0]!.children).toEqual([{ id: "t1", title: "Map", steps: 1, step: "Ran ls" }]);
  });
});
