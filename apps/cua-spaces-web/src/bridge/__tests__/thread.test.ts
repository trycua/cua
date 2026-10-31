// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Space thread (CUA-2155): the daemon tools' JSON mapped once for every
// host, the fold the page draws (task state from updates, replies nested,
// approvals answered, the unread divider), and an old daemon read as
// unsupported.

import { describe, expect, it } from "vitest";

import { HostError, UnsupportedOperationError } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import { DEMO_THREAD_SPACE } from "../adapters/demo/thread";
import { agentMembersFromTool, isMissingThreadTool, threadEntryFromTool, threadLiveFromTool, threadPageFromTool, threadPostArgs, threadReadArgs, threadSetRightsArgs, threadViewFromTool, webkitThreadOps, type ThreadEntry } from "../ops/thread";
import { foldThread, readSeqFrom } from "../thread";

let seq = 0;
function entry(e: Partial<ThreadEntry> & { author?: ThreadEntry["author"] }): ThreadEntry {
  seq += 1;
  return {
    seq,
    id: `e${seq}`,
    tsMs: seq * 1000,
    author: { kind: "person", id: "acct_ada", name: "Ada" },
    kind: "message",
    text: "",
    mentions: [],
    replyTo: null,
    task: null,
    attachments: [],
    approval: null,
    answer: null,
    noteFor: null,
    update: null,
    runsOnKeysOf: null,
    event: null,
    ...e,
  };
}
const CLAUDE = { kind: "agent" as const, id: "agent:claude", name: "claude" };
const GRACE = { kind: "person" as const, id: "acct_grace", name: "Grace" };
const SYSTEM = { kind: "system" as const, id: "system:dispatcher", name: "Cua" };
const update = (task: number, state: "claimed" | "working" | "done" | "failed" | "queued", note: string | null = null) =>
  entry({ author: SYSTEM, kind: "task_update", update: { task, state, session: 1, note } });

describe("the thread tools' JSON", () => {
  it("maps an entry, and reads unknown kinds as messages", () => {
    const page = threadPageFromTool({
      entries: [
        {
          v: 1,
          seq: 41,
          id: "01JB",
          ts_ms: 1761300000000,
          author: { kind: "person", id: "acct_7f", name: "Dillon" },
          kind: "message",
          text: "@claude run the e2e suite",
          mentions: ["claude"],
          reply_to: null,
          task: { to: "claude", session: null },
          attachments: [{ path: "spaces/dev/thread/files/01JB/report.html", name: "report.html", size: 212000, mime: "text/html" }],
          approval: null,
          update: null,
        },
        { seq: 42, kind: "reaction", author: { id: "agent:hermes" }, text: "👍" },
        { kind: "message", text: "no seq: dropped" },
      ],
      cursor: 42,
      caught_up: false,
      unread: 2,
      read_seq: 40,
      me: { id: "acct_7f" },
      has_more_before: true,
    });
    expect(page.hasMoreBefore).toBe(true);
    expect(threadPageFromTool({ entries: [{ seq: 3, kind: "system", task_seq: 1, text: "Hermes woke up" }] }).entries[0]!.noteFor).toBe(1);
    expect(threadPageFromTool({ entries: [] }).hasMoreBefore).toBe(false);
    expect(page.cursor).toBe(42);
    expect(page.caughtUp).toBe(false);
    expect(page.readSeq).toBe(40);
    expect(page.me).toBe("acct_7f");
    expect(page.entries).toHaveLength(2);
    const [task, other] = page.entries;
    expect(task!.task).toEqual({ to: "claude", session: null });
    expect(task!.attachments[0]).toEqual({ path: "spaces/dev/thread/files/01JB/report.html", name: "report.html", size: 212000, mime: "text/html" });
    expect(other!.kind).toBe("message");
    expect(other!.author).toEqual({ kind: "agent", id: "agent:hermes", name: "hermes" });
  });

  it("maps members with a status word or a status object", () => {
    expect(
      agentMembersFromTool({
        members: [
          { name: "claude", harness: "claude-code", status: "working", activity: "running e2e", sessions: 3 },
          { name: "hermes", harness: "hermes", status: { state: "asleep" } },
          { harness: "nameless" },
        ],
      }).members,
    ).toEqual([
      { name: "claude", harness: "claude-code", status: "working", activity: "running e2e", sessions: 3, rights: null, youCan: null, heldBy: null, identity: null },
      { name: "hermes", harness: "hermes", status: "asleep", activity: null, sessions: 1, rights: null, youCan: null, heldBy: null, identity: null },
    ]);
  });

  it("maps rights, what you can do, who has control, whose keys and your role (CUA-2156)", () => {
    const [m] = agentMembersFromTool({
      members: [
        {
          name: "claude",
          harness: "claude-code",
          status: { state: "paused", activity: "Mia has control" },
          rights: { task: ["acct_mia"], control: "owner", approve: "nobody" },
          you_can: { task: true, control: false, approve: false },
          held_by: { kind: "person", id: "acct_mia", name: "Mia" },
        },
      ],
    }).members;
    // An unknown value reads as the default.
    expect(m!.rights).toEqual({ task: ["acct_mia"], control: "owner", approve: "admins" });
    expect(m!.youCan).toEqual({ task: true, control: false, approve: false });
    expect(m!.heldBy).toBe("Mia");
    const page = threadPageFromTool({
      entries: [{ seq: 1, kind: "message", author: { kind: "person", id: "acct_mia", name: "Mia" }, text: "@claude go", task: { to: "claude" }, runs_on_keys_of: { id: "acct_ada", name: "Ada" } }],
      role: "guest",
    });
    expect(page.entries[0]!.runsOnKeysOf).toEqual({ id: "acct_ada", name: "Ada" });
    expect(page.role).toBe("guest");
    // An older daemon says neither.
    expect(threadPageFromTool({ entries: [], role: "superuser" }).role).toBeNull();
    expect(threadSetRightsArgs({ spaceId: "local:dev", name: "claude", approve: "members" })).toEqual({ name: "claude", approve: "members" });
  });

  it("builds the tools' arguments, bounding the long poll", () => {
    expect(threadReadArgs({ spaceId: "local:dev", after: 0, limit: 100, markRead: true, followMs: 90_000 })).toEqual({ space: "local:dev", after: 0, limit: 100, mark_read: true, follow_ms: 25_000 });
    expect(threadReadArgs({ spaceId: "local:dev" })).toEqual({ space: "local:dev" });
    // The stage key rides only a follow, so the wait ends when an agent's stage changes.
    expect(threadReadArgs({ spaceId: "local:dev", after: 3, followMs: 25_000, liveAfter: "claude=thinking" })).toEqual({ space: "local:dev", after: 3, follow_ms: 25_000, live_after: "claude=thinking" });
    expect(threadReadArgs({ spaceId: "local:dev", after: 3, liveAfter: "claude=thinking" })).toEqual({ space: "local:dev", after: 3 });
    expect(threadReadArgs({ spaceId: "local:dev", before: 40, limit: 100 })).toEqual({ space: "local:dev", before: 40, limit: 100 });
    expect(threadPostArgs({ spaceId: "local:dev", text: "Denied", answer: { approval: 9, approve: false } })).toEqual({ space: "local:dev", text: "Denied", answer: { approval: 9, approve: false } });
  });

  it("reads an old daemon's missing tool as unsupported, and nothing else", () => {
    expect(isMissingThreadTool(new UnsupportedOperationError("tauri", "thread.read"))).toBe(true);
    expect(isMissingThreadTool(new HostError("x", "unsupported"))).toBe(true);
    expect(isMissingThreadTool(new HostError("Spaces tool thread_read", "failed"))).toBe(true);
    expect(isMissingThreadTool(new Error("unknown tool agent_members"))).toBe(true);
    expect(isMissingThreadTool(new HostError("not_found: no Space local:gone", "not_found"))).toBe(false);
    expect(isMissingThreadTool(new Error("unknown tool volume_ls"))).toBe(false);
  });

  it("maps a native host's answer, and its missing tool to unsupported", async () => {
    const ops = webkitThreadOps((async (method: string) => {
      if (method === "thread.members") throw new HostError("NotFound: Spaces tool agent_members", "failed");
      return { entries: [], cursor: 7, caught_up: true, unread: 0 };
    }) as never);
    expect(await ops["thread.read"]({ spaceId: "local:dev" })).toMatchObject({ cursor: 7, entries: [] });
    await expect(ops["thread.members"]({ spaceId: "local:dev" })).rejects.toMatchObject({ code: "unsupported" });
  });
});

describe("folding the thread", () => {
  it("folds a task's state from its updates, and its reply is its own row, in order", () => {
    seq = 0;
    const task = entry({ text: "@claude run the suite", task: { to: "claude", session: null } });
    const entries = [
      task,
      update(task.seq, "claimed"),
      update(task.seq, "working"),
      entry({ author: CLAUDE, replyTo: task.seq, text: "146 passed, 2 failed" }),
      update(task.seq, "done"),
      entry({ author: GRACE, text: "nice" }),
    ];
    const items = foldThread(entries);
    // Flat: the task, its reply, then the next message, one row each, nothing nested.
    expect(items.map((i) => i.type)).toEqual(["task", "message", "message"]);
    const t = items[0] as Extract<(typeof items)[number], { type: "task" }>;
    expect(t.state).toBe("done");
    expect("replies" in t || "notes" in t).toBe(false);
    const reply = items[1] as Extract<(typeof items)[number], { type: "message" }>;
    expect(reply.entry.text).toBe("146 passed, 2 failed");
    // The reply knows what it answers (for a quote, and for Retry); an ordinary message does not.
    expect(reply.quote).toMatchObject({ seq: task.seq, text: "@claude run the suite" });
    expect(reply.taskText).toBe("@claude run the suite");
    expect((items[2] as Extract<(typeof items)[number], { type: "message" }>).quote).toBeNull();
  });

  it("draws a message that tasks several agents once, and reads its copy's marker from the tools' JSON", () => {
    seq = 0;
    const first = entry({ text: "@claude @hermes review", task: { to: "claude", session: null } });
    const copy = entry({ text: "@claude @hermes review", task: { to: "hermes", session: null }, fanoutOf: first.seq });
    const reply = entry({ author: { kind: "agent", id: "agent:hermes", name: "hermes" }, replyTo: copy.seq, text: "Looks good" });
    const items = foldThread([first, copy, update(copy.seq, "working"), reply]);
    expect(items.map((i) => i.type)).toEqual(["task", "message"]);
    expect((items[1] as Extract<(typeof items)[number], { type: "message" }>).entry.text).toBe("Looks good");
    const page = threadPageFromTool({
      entries: [{ seq: 3, kind: "message", author: { kind: "person", id: "a", name: "A" }, text: "x", task: { to: "hermes" }, fanout_of: 2 }, { seq: 2, kind: "message", author: { kind: "person", id: "a", name: "A" }, text: "x" }],
    });
    expect(page.entries.find((e) => e.seq === 3)?.fanoutOf).toBe(2);
    expect(page.entries.find((e) => e.seq === 2)?.fanoutOf).toBeUndefined();
  });

  it("keeps a task queued until it is picked up, and shows a failure", () => {
    seq = 0;
    const queued = entry({ task: { to: "claude", session: null } });
    const failed = entry({ task: { to: "hermes", session: null } });
    const items = foldThread([failed, update(failed.seq, "failed"), queued].reverse());
    expect(items.map((i) => (i.type === "task" ? i.state : i.type))).toEqual(["queued", "failed"]);
  });

  it("shows notes, an update's note, and an approval with its answer", () => {
    seq = 0;
    const ask = entry({ author: CLAUDE, kind: "approval", text: "Push?", approval: { action: "Push to main", details: "", state: "open" } });
    const task = entry({ task: { to: "claude", session: null } });
    const items = foldThread([
      ask,
      task,
      update(task.seq, "claimed", "Session 2 picked this up"),
      entry({ author: SYSTEM, kind: "system", text: "Hermes woke up and joined" }),
      entry({ author: GRACE, kind: "answer", text: "Approved", answer: { approval: ask.seq, approve: true, note: null } }),
    ]);
    // Notes are muted lines in sequence, whether or not they are about a task.
    expect(items.map((i) => (i.type === "system" ? i.text : i.type))).toEqual(["approval", "task", "Session 2 picked this up", "Hermes woke up and joined"]);
    const a = items[0] as Extract<(typeof items)[number], { type: "approval" }>;
    expect(a.answer).toMatchObject({ approve: true, by: GRACE });
  });

  it("puts a note about a task, and its reply, in sequence with everything else", () => {
    seq = 0;
    const task = entry({ author: CLAUDE, task: { to: "hermes", session: null } });
    const woke = entry({ author: SYSTEM, kind: "system", text: "Hermes woke up and joined", noteFor: task.seq });
    const reply = entry({ author: { kind: "agent", id: "agent:hermes", name: "hermes" }, replyTo: task.seq, text: "Done." });
    const claimed = update(task.seq, "claimed", "Session 2 picked this up");
    const elsewhere = entry({ author: SYSTEM, kind: "system", text: "about a task not on this page", noteFor: 999 });
    const items = foldThread([task, woke, reply, claimed, elsewhere]);
    expect(items.map((i) => (i.type === "system" ? i.text : i.type))).toEqual(["task", "Hermes woke up and joined", "message", "Session 2 picked this up", "about a task not on this page"]);
  });

  it("puts the unread divider before the first entry someone else wrote after the cursor", () => {
    seq = 0;
    const read = entry({ author: GRACE, text: "seen" });
    const mine = entry({ text: "my own, after the cursor" });
    const task = entry({ task: { to: "claude", session: null } });
    const reply = entry({ author: CLAUDE, replyTo: task.seq, text: "done" });
    const items = foldThread([read, mine, task, reply], { readSeq: read.seq, me: "acct_ada" });
    // Mine and my task are not unread; Claude's reply (its own row now) is, and the divider sits before it.
    expect(items.map((i) => i.type)).toEqual(["message", "message", "task", "unread", "message"]);
    expect(foldThread([read], { readSeq: read.seq, me: "acct_ada" }).some((i) => i.type === "unread")).toBe(false);
    expect(foldThread([read, mine], { readSeq: null }).some((i) => i.type === "unread")).toBe(false);
  });

  it("works out the reader's cursor from the unread count when the daemon does not say it", () => {
    seq = 0;
    const a = entry({ author: GRACE });
    entry({});
    const c = entry({ author: CLAUDE });
    const d = entry({ author: GRACE });
    const all = [a, { ...a, seq: 2, author: { kind: "person" as const, id: "acct_ada", name: "Ada" } }, c, d];
    expect(readSeqFrom(all, 2, "acct_ada")).toBe(c.seq - 1);
    expect(readSeqFrom(all, 0, "acct_ada")).toBe(d.seq);
    expect(readSeqFrom(all, 9, "acct_ada")).toBeNull();
  });
});

describe("the demo host's thread", () => {
  it("has the storyboard's conversation, and an agent answers its task", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 1 });
    const page = await demo.call("thread.read", { spaceId: DEMO_THREAD_SPACE, limit: 100 });
    const items = foldThread(page.entries, { readSeq: page.readSeq, me: page.me });
    const states = items.flatMap((i) => (i.type === "task" ? [i.state] : []));
    expect(states).toEqual(expect.arrayContaining(["done", "working", "queued"]));
    expect(items.some((i) => i.type === "approval" && i.answer === null)).toBe(true);
    expect(items.some((i) => i.type === "unread")).toBe(true);
    expect(items.some((i) => i.type === "system" && i.text === "Hermes woke up and joined")).toBe(true);
    expect(page.hasMoreBefore).toBe(false);
    const posted = await demo.call("thread.post", { spaceId: "local:design-review", text: "hello" });
    expect(posted?.text).toBe("hello");
    // A follow returns once the next entry is written.
    const next = demo.call("thread.read", { spaceId: "local:design-review", after: posted!.seq, followMs: 5_000 });
    await demo.call("thread.post", { spaceId: "local:design-review", text: "again" });
    expect((await next).entries.map((e) => e.text)).toEqual(["again"]);
    demo.dispose?.();
  });

  it("plays rights: a guest reads only, a member can't task Hermes or approve, the owner sets rights", async () => {
    const guest = createDemoAdapter({ latencyMs: 0, stepMs: 1, threadRole: "guest" });
    expect((await guest.call("thread.read", { spaceId: DEMO_THREAD_SPACE, limit: 5 })).role).toBe("guest");
    await expect(guest.call("thread.post", { spaceId: DEMO_THREAD_SPACE, text: "hi" })).rejects.toMatchObject({ code: "permission_denied" });
    guest.dispose?.();

    const member = createDemoAdapter({ latencyMs: 0, stepMs: 1, threadRole: "member" });
    const { members } = await member.call("thread.members", { spaceId: DEMO_THREAD_SPACE });
    expect(members.find((m) => m.name === "hermes")!.youCan).toEqual({ task: false, control: false, approve: false });
    const page = await member.call("thread.read", { spaceId: DEMO_THREAD_SPACE, limit: 100 });
    const approval = page.entries.find((e) => e.kind === "approval")!;
    await expect(member.call("thread.post", { spaceId: DEMO_THREAD_SPACE, text: "ok", answer: { approval: approval.seq, approve: true } })).rejects.toMatchObject({
      code: "permission_denied",
    });
    // A task you give runs on the owner's keys; a mention of Hermes makes none.
    const task = await member.call("thread.post", { spaceId: DEMO_THREAD_SPACE, text: "@claude go" });
    expect(task!.runsOnKeysOf).toEqual({ id: "acct_grace", name: "Grace" });
    const plain = await member.call("thread.post", { spaceId: DEMO_THREAD_SPACE, text: "@hermes go" });
    expect(plain!.task).toBeNull();
    await expect(member.call("thread.setRights", { spaceId: DEMO_THREAD_SPACE, name: "hermes", task: "members" })).rejects.toMatchObject({ code: "permission_denied" });
    member.dispose?.();

    const owner = createDemoAdapter({ latencyMs: 0, stepMs: 1 });
    const hermes = await owner.call("thread.setRights", { spaceId: DEMO_THREAD_SPACE, name: "hermes", task: "members" });
    expect(hermes!.rights!.task).toBe("members");
    const last = (await owner.call("thread.read", { spaceId: DEMO_THREAD_SPACE, limit: 1 })).entries[0]!;
    expect(last.text).toBe("Ada changed who can use hermes. task: the owner and members");
    owner.dispose?.();
  });

  it("plays a daemon without the thread", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, noThread: true });
    await expect(demo.call("thread.read", { spaceId: DEMO_THREAD_SPACE })).rejects.toSatisfy(isMissingThreadTool);
    demo.dispose?.();
  });
});

describe("join codes and join requests (CUA-2263)", () => {
  it("maps the tools' arguments", async () => {
    const ops = await import("../ops/thread");
    expect(ops.threadJoinCodeCreateArgs({ spaceId: "local:dev" })).toEqual({ space: "local:dev" });
    expect(ops.threadJoinCodeCreateArgs({ spaceId: "local:dev", ttlS: 900, canTask: "members", askBeforeJoin: true })).toEqual({
      space: "local:dev",
      ttl_s: 900,
      can_task: "members",
      ask_before_join: true,
    });
    expect(ops.threadJoinCodeCancelArgs({ spaceId: "local:dev", id: "jc_1" })).toEqual({ space: "local:dev", id: "jc_1" });
    expect(ops.threadSettingsArgs({ spaceId: "local:dev" })).toEqual({ space: "local:dev" });
    expect(ops.threadSettingsArgs({ spaceId: "local:dev", askBeforeJoin: false })).toEqual({ space: "local:dev", ask_before_join: false });
    expect(ops.threadAgentAddArgs({ spaceId: "local:dev", name: "codex", harness: "openai-codex" })).toEqual({ space: "local:dev", name: "codex", agent: "openai-codex" });
    expect(ops.THREAD_TOOLS["thread.joinCodeList"].tool).toBe("thread_join_code_list");
  });

  it("reads codes, settings and a join request as the daemon writes them", async () => {
    const ops = await import("../ops/thread");
    const made = ops.joinCodeFromTool({ id: "jc_1", code: "7KQ2-9FXA", url: "https://cua.ai/join/7KQ2-9FXA", line: 'Join my Cua Space "dev": https://cua.ai/join/7KQ2-9FXA', expires_at_ms: 9 });
    expect(made).toEqual({ id: "jc_1", code: "7KQ2-9FXA", url: "https://cua.ai/join/7KQ2-9FXA", line: 'Join my Cua Space "dev": https://cua.ai/join/7KQ2-9FXA', expiresAtMs: 9 });
    const listed = ops.joinCodesFromTool({
      codes: [
        { id: "jc_1", created_at_ms: 1, expires_at_ms: 2, state: "redeemed", can_task: "members", ask_before_join: true, created_by: { id: "acct_ada", name: "Ada" }, member_handle: "codex" },
        { id: "jc_2", created_at_ms: 1, expires_at_ms: 2, state: "weird" },
        { nope: true },
      ],
    });
    expect(listed.codes).toEqual([
      { id: "jc_1", createdAtMs: 1, expiresAtMs: 2, state: "redeemed", canTask: "members", askBeforeJoin: true, createdBy: { id: "acct_ada", name: "Ada" }, memberHandle: "codex" },
      { id: "jc_2", createdAtMs: 1, expiresAtMs: 2, state: "expired", canTask: "owner", askBeforeJoin: false, createdBy: null, memberHandle: null },
    ]);
    expect(ops.joinCodesFromTool([{ id: "jc_3", state: "open" }]).codes[0]!.state).toBe("open");
    expect(ops.threadSettingsFromTool({ ask_before_join: false })).toEqual({ askBeforeJoin: false });
    expect(ops.threadSettingsFromTool({})).toEqual({ askBeforeJoin: true });
    const identity = {
      origin: "external",
      handle: "agent",
      display_name: "Agent",
      icon: null,
      harness: { id: "other", version: null, reported: false },
      owner: { id: "acct_ada", name: "Ada" },
      device: { id: "dev_1", name: "Ada's MacBook Pro" },
      joined: { method: "prompt", at_ms: 5, by: { id: "acct_ada", name: "Ada" } },
      state: "pending",
      last_seen_ms: 5,
    };
    const e = ops.threadEntryFromTool({
      seq: 4,
      id: "x",
      ts_ms: 5,
      author: { kind: "system", id: "system:join", name: "Cua" },
      kind: "join_request",
      text: "Ada's agent asks to join from Ada's MacBook Pro.",
      join_request: { member: identity, can_task: "members", state: "open", code_id: "jc_1" },
      you_can_answer: true,
    });
    expect(e!.kind).toBe("join_request");
    expect(e!.joinRequest).toMatchObject({ canTask: "members", state: "open", codeId: "jc_1", youCanAnswer: true });
    expect(e!.joinRequest!.member).toMatchObject({ origin: "external", state: "pending", device: { name: "Ada's MacBook Pro" } });
  });
});

describe("the view a post was sent from", () => {
  it("goes to thread_post in the tool's names and comes back on the entry", () => {
    const view = { kind: "window" as const, app: "Firefox", title: "Pull requests", windowId: "w42" };
    expect(threadPostArgs({ spaceId: "local:dev", text: "this one?", view })).toEqual({
      space: "local:dev",
      text: "this one?",
      view: { kind: "window", app: "Firefox", title: "Pull requests", window_id: "w42" },
    });
    // Only the fields there are.
    expect(threadPostArgs({ spaceId: "local:dev", text: "x", view: { kind: "window", app: null, title: "notes.txt", windowId: null } }).view).toEqual({ kind: "window", title: "notes.txt" });
    expect(threadPostArgs({ spaceId: "local:dev", text: "x" })).not.toHaveProperty("view");
    const e = threadEntryFromTool({
      seq: 4,
      kind: "message",
      text: "this one?",
      author: { kind: "person", id: "owner", name: "Ada" },
      view: { kind: "window", app: "Firefox", title: "Pull requests", window_id: "w42" },
    });
    expect(e?.view).toEqual(view);
    // An entry without one reads as before; an unknown kind is dropped.
    expect(threadEntryFromTool({ seq: 5, kind: "message", text: "hi", author: { kind: "person", id: "o", name: "A" } })).not.toHaveProperty("view");
    expect(threadViewFromTool({ kind: "tab" })).toBeNull();
    expect(threadViewFromTool({ kind: "desktop" })).toEqual({ kind: "desktop", app: null, title: null, windowId: null });
  });
});

describe("the live rows' JSON", () => {
  it("reads who is at work and the key that changes with it; a daemon without it says nobody", () => {
    expect(threadLiveFromTool({ key: "ada=using Bash;bo=thinking", agents: [{ agent: "ada", stage: "using Bash" }, { agent: "bo", stage: "thinking" }, { agent: "", stage: "x" }, { stage: "y" }] })).toEqual({
      key: "ada=using Bash;bo=thinking",
      agents: [
        { agent: "ada", stage: "using Bash" },
        { agent: "bo", stage: "thinking" },
      ],
    });
    expect(threadPageFromTool({ entries: [], cursor: 0 }).live).toEqual({ key: "", agents: [] });
    expect(threadPageFromTool({ entries: [], cursor: 0, live: { key: "k", agents: [{ agent: "a", stage: "starting" }] } }).live.agents).toEqual([{ agent: "a", stage: "starting" }]);
  });
});
