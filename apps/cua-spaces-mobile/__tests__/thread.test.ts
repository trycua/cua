// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { foldThread, readSeqFrom } from "../src/data/thread/fold";
import { threadEntryFromTool, threadPageFromTool, threadReadArgs, threadPostArgs } from "../src/data/thread/protocol";
import { entry, ME, SAM, CLAUDE, CUA } from "../src/data/demo/data";

describe("the thread wire (same mapping as the web app's bridge/ops/thread.ts)", () => {
  it("maps tool JSON (snake_case) to an entry, and reads a card only from Cua's own host", () => {
    const base = { seq: 7, id: "e7", ts_ms: 5, text: "wants to push", kind: "approval", approval: { action: "export:repo.push", details: "x", state: "open", card: { repo: "acme/app" } } };
    const fromHost = threadEntryFromTool({ ...base, author: { kind: "system", id: "system:host", name: "Cua" } });
    expect(fromHost?.approval?.card).toEqual({ repo: "acme/app" });
    const fromAgent = threadEntryFromTool({ ...base, author: { kind: "agent", id: "agent:claude", name: "claude" } });
    expect(fromAgent?.approval?.card).toBeNull();
  });

  it("maps a page with its cursor, live agents and role", () => {
    const p = threadPageFromTool({ entries: [], cursor: 12, caught_up: true, unread: 2, role: "owner", me: { id: "acct:you" }, live: { key: "k1", agents: [{ agent: "@claude", stage: "using Bash", since_ms: 9, text: "Running" }] } });
    expect(p.cursor).toBe(12);
    expect(p.role).toBe("owner");
    expect(p.me).toBe("acct:you");
    expect(p.live.agents[0]).toEqual({ agent: "@claude", stage: "using Bash", sinceMs: 9, text: "Running" });
  });

  it("builds the tool arguments the daemon takes", () => {
    expect(threadReadArgs({ spaceId: "relay:a", after: 3, limit: 100, markRead: true, followMs: 99999, liveAfter: "k" })).toEqual({ space: "relay:a", after: 3, limit: 100, mark_read: true, follow_ms: 25000, live_after: "k" });
    expect(threadPostArgs({ spaceId: "relay:a", text: "Approved", answer: { approval: 7, approve: true } })).toEqual({ space: "relay:a", text: "Approved", answer: { approval: 7, approve: true } });
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
    const answered = foldThread([...entries, entry(4, t0 + 3, ME, "Approved", { kind: "answer", answer: { approval: 3, approve: true, note: null } })], { me: ME.id });
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
