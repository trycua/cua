// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import {
  mergeTaskOptions,
  mergeToolRows,
  taskOptionsFromTool,
  taskResultFromTool,
  toolRowsFromTool,
  changesFromTool,
  fileDiffFromTool,
  filePreviewFromTool,
  toolRowsArgs,
} from "../src/data/thread/phone-protocol";
import { threadEntryFromTool, threadPostArgs, agentMembersFromTool } from "../src/data/thread/protocol";
import { createDemoBackend } from "../src/data/demo/backend";
import { CHECKOUT_TOOL_ROWS_WIRE } from "../src/data/demo/data";

describe("tool rows, as the daemon sends them (thread_tool_rows)", () => {
  it("reads groups and calls, with a state for each call", () => {
    const p = toolRowsFromTool({
      groups: [
        {
          after_seq: 4,
          agent: "claude",
          calls: [{ id: "a", cat: "ran", line: "Ran ls", detail: "ls", state: "running", at_ms: 3 }, { id: "b", line: "Read x" }, { line: "" }],
        },
        { after_seq: "x" },
      ],
      cursor: 9,
      caught_up: false,
    });
    expect(p.cursor).toBe(9);
    expect(p.caughtUp).toBe(false);
    expect(p.groups).toHaveLength(1);
    expect(p.groups[0]!.calls).toEqual([
      { id: "a", cat: "ran", line: "Ran ls", detail: "ls", state: "running", atMs: 3 },
      { id: "b", cat: "other", line: "Read x", detail: "", state: "done", atMs: 0 },
    ]);
  });

  it("builds the arguments with a cursor and a long poll", () => {
    expect(toolRowsArgs({ spaceId: "relay:a", cursor: 7, followMs: 15000 })).toEqual({ space: "relay:a", cursor: 7, follow_ms: 15000 });
    expect(toolRowsArgs({ spaceId: "relay:a" })).toEqual({ space: "relay:a" });
  });

  it("a poll that repeats a call updates it (running to done) and a new call is added in order", () => {
    const first = toolRowsFromTool({
      groups: [{ after_seq: 4, agent: "claude", calls: [{ id: "a", cat: "ran", line: "Ran npm test", state: "running" }] }],
    }).groups;
    const next = toolRowsFromTool({
      groups: [
        {
          after_seq: 4,
          agent: "claude",
          calls: [
            { id: "a", cat: "ran", line: "Ran npm test", state: "done" },
            { id: "b", cat: "read", line: "Read x" },
          ],
        },
        { after_seq: 2, agent: "claude", calls: [{ id: "z", cat: "ran", line: "Ran ls" }] },
      ],
    }).groups;
    const merged = mergeToolRows(first, next);
    expect(merged.map((g) => g.afterSeq)).toEqual([2, 4]);
    expect(merged[1]!.calls.map((c) => [c.id, c.state])).toEqual([
      ["a", "done"],
      ["b", "done"],
    ]);
  });

  it("the sample backend answers in the same wire shape, read by the same mapper as a live answer", async () => {
    const wire = toolRowsFromTool(CHECKOUT_TOOL_ROWS_WIRE);
    const demo = createDemoBackend({ now: () => 1_800_000_000_000 });
    const page = await demo.toolRows!("relay:sample-checkout-api");
    expect(page.groups).toEqual(wire.groups);
    expect(page.groups[0]!.calls.map((c) => c.cat)).toEqual(["read", "read", "edited", "edited", "ran"]);
  });
});

describe("starting a task, changes and diffs (the wire of INTERFACES.md)", () => {
  it("reads what one machine can start: itself, and which agents are connected there", () => {
    const o = taskOptionsFromTool({
      machine: { id: "m1", os: ["macos"], online: true, can_create: true },
      agents: [
        { harness: "claude-code", handle: "claude", label: "Claude Code", ready: true },
        { harness: "openai-codex", label: "Codex", ready: false, why: "Sign in" },
        {},
      ],
    });
    expect(o.machines).toEqual([{ id: "m1", name: "m1", online: true, canCreate: true, why: null, os: ["macos"], readyAgents: ["claude-code"] }]);
    expect(o.agents).toEqual([
      { harness: "claude-code", label: "Claude Code", ready: true, why: null },
      { harness: "openai-codex", label: "Codex", ready: false, why: "Sign in" },
    ]);
    // A list of machines is read too.
    const list = taskOptionsFromTool({ machines: [{ id: "m2", name: "Desk PC", online: false, why: "no room" }, {}], agents: [] });
    expect(list.machines).toEqual([{ id: "m2", name: "Desk PC", online: false, canCreate: false, why: "no room", os: [], readyAgents: null }]);
  });

  it("merges every machine's answer: an agent is ready if some machine has it, and names come from the relay", () => {
    const a = taskOptionsFromTool({
      machine: { id: "m1", os: ["macos"], online: true, can_create: true },
      agents: [
        { harness: "claude-code", label: "Claude Code", ready: false, why: "Sign in" },
        { harness: "hermes", label: "Hermes", ready: true },
      ],
    });
    const b = taskOptionsFromTool({
      machine: { id: "m2", os: ["linux"], online: true, can_create: false, why: "Budget reached" },
      agents: [{ harness: "claude-code", label: "Claude Code", ready: true }],
    });
    const m = mergeTaskOptions(
      [a, b],
      new Map([
        ["m1", "Studio Mac"],
        ["m2", "Linked cloud"],
      ]),
    );
    expect(m.machines.map((x) => [x.id, x.name, x.canCreate, x.readyAgents])).toEqual([
      ["m1", "Studio Mac", true, ["hermes"]],
      ["m2", "Linked cloud", false, ["claude-code"]],
    ]);
    expect(m.agents).toEqual([
      { harness: "claude-code", label: "Claude Code", ready: true, why: null },
      { harness: "hermes", label: "Hermes", ready: true, why: null },
    ]);
  });

  it("reads where a task went, a task still being made, a failure, and refuses an answer that does not say", () => {
    expect(taskResultFromTool({ space: "relay:s1", created: true, entry_seq: 3, state: "ready", agent: "claude" })).toEqual({
      space: "relay:s1",
      created: true,
      entrySeq: 3,
      state: "ready",
      pendingId: null,
      error: null,
      agent: "claude",
    });
    expect(taskResultFromTool({ space: null, state: "creating", pending_id: "p1" })).toMatchObject({ space: "", state: "creating", pendingId: "p1" });
    expect(taskResultFromTool({ state: "failed", error: "no room" })).toMatchObject({ state: "failed", error: "no room" });
    expect(taskResultFromTool({ space: "relay:s1", created: false })).toMatchObject({ created: false, state: "ready" });
    expect(() => taskResultFromTool({})).toThrow(/did not say where/);
  });

  it("reads the change set, one file's diff and a preview", () => {
    const c = changesFromTool({
      repo: "a/b",
      branch: "x",
      base: "main",
      head_sha: "abc",
      verified: true,
      files: [{ path: "a.ts", status: "added", additions: 3, deletions: 0, size: 5, mime: "text/plain" }, { path: "p.png", binary: true }, { nope: 1 }],
      truncated: true,
    });
    expect(c.files).toHaveLength(2);
    expect(c).toMatchObject({ repo: "a/b", base: "main", headSha: "abc", truncated: true, verified: true });
    expect(fileDiffFromTool({ path: "a.ts", patch: "@@", truncated: true })).toEqual({ path: "a.ts", patch: "@@", truncated: true, binary: false });
    expect(filePreviewFromTool({ path: "p.png", mime: "image/png", size: 9, encoding: "base64", content: "AAA" })).toMatchObject({
      encoding: "base64",
      content: "AAA",
      truncated: false,
    });
  });
});

describe("the thread wire gained the client id, the scope and the queue", () => {
  it("sends the client id and the scope of an allow inside the answer, never a scope on a decline", () => {
    expect(
      threadPostArgs({ spaceId: "relay:a", text: "Allowed for this Space", answer: { approval: 8, approve: true, scope: "space", clientId: "c1" } }),
    ).toEqual({ space: "relay:a", text: "Allowed for this Space", answer: { approval: 8, approve: true, scope: "space", client_id: "c1" } });
    expect(threadPostArgs({ spaceId: "relay:a", text: "Denied", answer: { approval: 8, approve: false, scope: "space" } }).answer).toEqual({
      approval: 8,
      approve: false,
    });
  });

  it("reads an approval's kind and scope choices (structured data) and an answer's scope", () => {
    const e = threadEntryFromTool({
      seq: 8,
      id: "e8",
      ts_ms: 1,
      kind: "approval",
      text: "t",
      author: { kind: "agent", id: "agent:c", name: "c" },
      approval: { action: "install a package", details: "d", state: "open", kind: "request", scope_choices: ["once", "space", "forever"] },
      client_id: "k",
    });
    expect(e?.approval).toMatchObject({ kind: "request", scopeChoices: ["once", "space"] });
    expect(e?.clientId).toBe("k");
    const a = threadEntryFromTool({
      seq: 9,
      id: "e9",
      ts_ms: 1,
      kind: "answer",
      text: "x",
      author: { kind: "person", id: "a", name: "a" },
      answer: { approval: 8, approve: true, scope: "space", client_id: "c3" },
    });
    expect(a?.answer).toMatchObject({ approve: true, scope: "space", clientId: "c3" });
  });

  it("reads how many messages wait behind a member's running turn", () => {
    const m = agentMembersFromTool({
      members: [
        { name: "claude", harness: "claude-code", status: "working", queued: 2 },
        { name: "x", status: "idle" },
      ],
    }).members;
    expect(m[0]!.queued).toBe(2);
    expect(m[1]!.queued).toBeUndefined();
  });
});
