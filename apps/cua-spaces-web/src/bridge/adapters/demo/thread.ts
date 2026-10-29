// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's Space threads, in the daemon tools' own JSON
 * (`thread_read`, `thread_post`, `agent_members`, DESIGN §3.2), mapped by
 * the same functions a native host's answer goes through
 * (`ops/thread.ts`). `ubuntu-build` has a conversation with people, Claude
 * Code and Hermes: tasks queued, working and done, a reply with
 * attachments, a system note, an open approval and unread entries. Every
 * other Space starts empty. `?demo=nothread` plays a daemon without the
 * thread (the Space page keeps its old layout).
 *
 * Posting works: a message that @mentions an agent becomes its task, which
 * the agent picks up and answers a few steps later.
 *
 * Rights (CUA-2156): you are the owner by default; `?demo=thread-member`
 * and `?demo=thread-guest` make you someone the Space is shared with (Grace
 * owns it then). A guest reads only; Hermes takes tasks from the owner only;
 * tasks from anyone but the owner run on the owner's keys.
 *
 * Identity (CUA-2262), in `agent_members`' `identity` and on each agent
 * author: the owner's Hermes started in the Space, Grace's Claude Code
 * connected from her MacBook with a prompt, and Linus's Cursor from his
 * ThinkPad. `?demo=thread-lookalike` adds Linus's look-alike that calls
 * itself "Claude Code" (`@claude-2`, harness not reported) and asks for a
 * key. Removing a member writes the daemon's note and keeps its posts.
 *
 * Add an agent (CUA-2264): Start an agent here adds a started member;
 * join codes are made, listed and cancelled, and an open code is redeemed
 * a few seconds after it is made by Ada's Codex from her MacBook Pro (its
 * join note, then its hello). With "Ask me before it joins" the redeem
 * becomes a join request to answer in the thread. `?demo=thread-join-slow`
 * never redeems (the sheet keeps waiting); `?demo=thread-join-expire`
 * makes codes expire after a few seconds; `?demo=thread-request` opens on
 * Linus's OpenClaw asking to join.
 */

import { HostError, UnsupportedOperationError } from "../../adapter";
import {
  agentMemberFromTool,
  agentMembersFromTool,
  joinCodeFromTool,
  joinCodesFromTool,
  DEFAULT_AGENT_RIGHTS,
  threadPageFromTool,
  threadPostFromTool,
  threadViewArgs,
  THREAD_FOLLOW_MAX_MS,
  THREAD_OPERATIONS,
  type AgentRight,
  type AgentRights,
  type RightHolders,
  type ThreadOpName,
} from "../../ops/thread";
import type { DemoHandlers } from "./context";

type Json = Record<string, unknown>;

/** What the pasted join line tells an agent to do next (the daemon's `line`, in `join.rs`). */
export const JOIN_LINE_STEPS = "After you join: set your name with thread_join, keep watching the thread with thread_inbox in a loop, reply with thread_post as yourself, and never @mention yourself.";

/** The demo's own account in the thread (`DEMO_IDENTITY`). */
export const DEMO_THREAD_ME = { kind: "person", id: "acct_ada", name: "Ada" } as const;
const GRACE = { kind: "person", id: "acct_grace", name: "Grace" } as const;
const LINUS = { kind: "person", id: "acct_linus", name: "Linus" } as const;

/** A member's `identity` in the daemon's JSON (CONTRACT §1). */
export interface DemoIdentity {
  origin: "space" | "external";
  handle: string;
  display_name: string;
  collides: boolean;
  icon: string | null;
  harness: { id: string; version: string | null; reported: boolean };
  owner: { id: string; name: string };
  device: { id: string; name: string } | null;
  joined: { method: "started" | "prompt"; at_ms: number; by: { id: string; name: string } };
  state: "active" | "pending";
  last_seen_ms: number | null;
}

const ref = (p: { id: string; name: string }) => ({ id: p.id, name: p.name });
const DAY = 86_400_000;

/** The members' identities, as the daemon keeps them (the owner and device come from its record, not the agent). */
function demoIdentities(now: number, owner: { id: string; name: string }): Record<string, DemoIdentity> {
  const at = (msAgo: number) => now - msAgo;
  return {
    hermes: {
      origin: "space",
      handle: "hermes",
      display_name: "Hermes",
      collides: false,
      icon: "harness",
      // Cua started it: the harness is known, not reported.
      harness: { id: "hermes", version: "0.9.2", reported: false },
      owner: ref(owner),
      device: null,
      joined: { method: "started", at_ms: at(3 * DAY + 4 * 3_600_000), by: ref(owner) },
      state: "active",
      last_seen_ms: null,
    },
    claude: {
      origin: "external",
      handle: "claude",
      display_name: "Claude Code",
      collides: false,
      icon: "harness",
      harness: { id: "claude-code", version: "2.4.1", reported: true },
      owner: ref(GRACE),
      device: { id: "dev_grace_mbp", name: "Grace's MacBook Pro" },
      // Yesterday at 10:30 local, whatever the time now (now - 26 h is two days ago just after midnight).
      joined: { method: "prompt", at_ms: new Date(new Date(now).getFullYear(), new Date(now).getMonth(), new Date(now).getDate() - 1, 10, 30).getTime(), by: ref(GRACE) },
      state: "active",
      last_seen_ms: at(60_000),
    },
    cursor: {
      origin: "external",
      handle: "cursor",
      display_name: "Cursor",
      collides: false,
      icon: "harness",
      harness: { id: "cursor", version: "1.7.2", reported: true },
      owner: ref(LINUS),
      device: { id: "dev_linus_tp", name: "linus-thinkpad" },
      joined: { method: "prompt", at_ms: at(5 * 3_600_000), by: ref(LINUS) },
      state: "active",
      // A CLI session that only runs when prompted: it last asked for its inbox a while ago.
      last_seen_ms: at(40 * 60_000),
    },
    "claude-2": {
      origin: "external",
      handle: "claude-2",
      display_name: "Claude Code",
      collides: true,
      // It uses Claude Code's look but reports no harness.
      icon: "harness",
      harness: { id: "other", version: null, reported: false },
      owner: ref(LINUS),
      device: { id: "dev_linus_tp", name: "linus-thinkpad" },
      joined: { method: "prompt", at_ms: at(6 * 60_000), by: ref(LINUS) },
      state: "active",
      last_seen_ms: at(60_000),
    },
  };
}

/** The Space with the conversation. */
export const DEMO_THREAD_SPACE = "relay:linux-box/ubuntu-build";
const FOLDER = "spaces/linux-box-ubuntu-build/thread/files";

interface DemoMember {
  name: string;
  harness: string;
  status: string;
  activity: string | null;
  sessions: number;
  rights: AgentRights;
  identity: DemoIdentity;
  /** A failed member's typed failure (the daemon's `status.failure`). */
  failure?: Json;
}

export type DemoThreadRole = "owner" | "member" | "guest";

interface DemoThread {
  entries: Json[];
  readSeq: number;
  members: DemoMember[];
  codes?: DemoCode[];
  /** DM read cursors: `<reader>\u0000dm:<handle>` -> seq (CUA-2266). */
  dmRead?: Map<string, number>;
  /** Join requests waiting for an answer: entry seq -> the member that would join. */
  requests?: Map<number, { member: DemoMember; code: DemoCode | null }>;
  askBeforeJoin?: boolean;
}

interface DemoCode {
  id: string;
  code: string;
  created_at_ms: number;
  expires_at_ms: number;
  state: "open" | "pending_approval" | "redeemed" | "denied" | "expired" | "cancelled";
  can_task: "owner" | "members";
  ask_before_join: boolean;
  created_by: { id: string; name: string };
  member_handle?: string;
}

/** What harness versions the demo's started agents report. */
const DEMO_VERSIONS: Record<string, string> = { hermes: "0.9.2", "claude-code": "2.4.1", codex: "0.61.0", cursor: "1.7.2", openclaw: "2026.10" };
const DEMO_LABELS: Record<string, string> = { hermes: "Hermes", "claude-code": "Claude Code", codex: "Codex", cursor: "Cursor", openclaw: "OpenClaw", t3code: "T3 Code" };

/** The DM between `me` and agent `with` (CUA-2266): the daemon's rule
 * (`thread::dm_entries`), so the demo pages the same entries a daemon does. */
export function demoDmEntries(entries: readonly Json[], me: string, with_: string): Json[] {
  const agent = `agent:${with_}`;
  const seqs = new Set<number>();
  const out: Json[] = [];
  const has = (v: unknown) => typeof v === "number" && seqs.has(v);
  for (const e of entries) {
    const author = (e.author as Json).id;
    const task = e.task as Json | null;
    const update = e.update as Json | null;
    const answer = e.answer as Json | null;
    const ev = e.event as Json | undefined;
    const keep =
      author === agent ||
      (author === me && (task?.to === with_ || ((e.mentions as string[]) ?? []).includes(with_))) ||
      has(e.reply_to) ||
      has(update?.task) ||
      has(answer?.approval) ||
      (e.kind === "system" && (has(e.task_seq) || (ev?.member as Json | undefined)?.handle === with_));
    if (keep) {
      seqs.add(e.seq as number);
      out.push(e);
    }
  }
  return out;
}

/** release-checks and design-review: Ada's Hermes asks to tag a release; Ada's Codex asks which branch (Agents tab, pp. 17–19). */
function demoSideThreads(now: number, me: { id: string; name: string }): [string, DemoThread][] {
  const min = (m: number) => now - m * 60_000;
  const build = (space: string, folder: string, fill: (add: (e: Json) => number) => DemoMember[], readAt: number): [string, DemoThread] => {
    let seq = 0;
    const entries: Json[] = [];
    const add = (e: Json) => {
      seq += 1;
      entries.push({ v: 1, seq, id: `01JBSIDE${folder}${String(seq).padStart(3, "0")}`, kind: "message", text: "", mentions: [], reply_to: null, task: null, attachments: [], approval: null, update: null, ...e });
      return seq;
    };
    const members = fill(add);
    return [space, { entries, readSeq: Math.min(readAt, seq), members }];
  };
  const sys = { kind: "system", id: "system:dispatcher", name: "Cua" };
  const ident = (handle: string, name: string, harness: string, origin: "space" | "external", joinedMin: number): DemoIdentity => ({
    origin,
    handle,
    display_name: name,
    collides: false,
    icon: "harness",
    harness: { id: harness, version: DEMO_VERSIONS[harness] ?? null, reported: origin === "external" },
    owner: ref(me),
    device: origin === "external" ? { id: "dev_ada_mbp", name: "Ada's MacBook Pro" } : null,
    joined: { method: origin === "external" ? "prompt" : "started", at_ms: min(joinedMin), by: ref(me) },
    state: "active",
    last_seen_ms: origin === "external" ? min(1) : null,
  });
  return [
    build("relay:mac-mini/release-checks", "RC", (add) => {
      const id = ident("hermes", "Hermes", "hermes", "space", 3 * 24 * 60);
      const H = { kind: "agent", id: "agent:hermes", name: "hermes", identity: id };
      const t = add({ ts_ms: min(9), author: me, text: "@hermes draft the 0.9.4 release notes from the changelog", mentions: ["hermes"], task: { to: "hermes", session: null } });
      add({ ts_ms: min(9), author: sys, kind: "task_update", update: { task: t, state: "working", session: 1, note: null } });
      add({
        ts_ms: min(7),
        author: H,
        reply_to: t,
        text: "Drafted: 2 fixes and 1 feature, from `changelog.md` in ubuntu-build.",
        attachments: [{ path: "spaces/release-checks/thread/files/01JBSIDERC003/notes-0.9.4.md", name: "notes-0.9.4.md", size: 2_000, mime: "text/markdown" }],
      });
      add({ ts_ms: min(7), author: sys, kind: "task_update", update: { task: t, state: "done", session: 1, note: null } });
      add({ ts_ms: min(3), author: H, kind: "approval", text: "Can I tag v0.9.4 and publish the notes?", approval: { action: "Tag v0.9.4 and publish a release", details: "git tag v0.9.4 && gh release create v0.9.4 -F notes-0.9.4.md", state: "open" } });
      return [{ name: "hermes", harness: "hermes", status: "waiting", activity: "Waiting for you in release-checks", sessions: 1, rights: { ...DEFAULT_AGENT_RIGHTS }, identity: id }];
    }, 2),
    build("local:design-review", "DR", (add) => {
      const id = ident("codex", "Codex", "codex", "external", 2 * 60);
      const C = { kind: "agent", id: "agent:codex", name: "codex", identity: id };
      const t = add({ ts_ms: min(120), author: me, text: "@codex update the Playwright snapshots for the new grid", mentions: ["codex"], task: { to: "codex", session: null } });
      add({ ts_ms: min(119), author: sys, kind: "task_update", update: { task: t, state: "working", session: 1, note: null } });
      add({ ts_ms: min(95), author: C, reply_to: t, text: "Updated 14 snapshots. All 212 tests pass.", attachments: [{ path: "spaces/design-review/thread/files/01JBSIDEDR003/snapshots.patch", name: "snapshots.patch", size: 38_000, mime: "text/x-patch" }] });
      add({ ts_ms: min(95), author: sys, kind: "task_update", update: { task: t, state: "done", session: 1, note: null } });
      add({ ts_ms: min(6), author: C, text: "@ada which branch should the snapshots go to, `main` or `grid-v2`?", mentions: ["ada"] });
      return [{ name: "codex", harness: "codex", status: "waiting", activity: "Waiting for you in design-review", sessions: 1, rights: { ...DEFAULT_AGENT_RIGHTS, approve: [me.id] }, identity: id }];
    }, 4),
  ];
}

function demoConversation(now: number, owner: { id: string; name: string }, opts: { lookalike?: boolean; authFailed?: boolean; stranger?: boolean; paused?: boolean } = {}): DemoThread {
  const ids = demoIdentities(now, owner);
  // An agent's posts carry who it was when it posted (they stay after it leaves).
  const agent = (handle: string) => ({ kind: "agent", id: `agent:${handle}`, name: handle, identity: ids[handle] });
  const CLAUDE = agent("claude");
  const HERMES = agent("hermes");
  // A task anyone but the owner gives runs on the owner's provider keys.
  const keys = (by: { id: string }) => (by.id === owner.id ? {} : { runs_on_keys_of: { id: owner.id, name: owner.name } });
  const min = (m: number) => now - m * 60_000;
  let seq = 0;
  const entries: Json[] = [];
  const add = (e: Json) => {
    seq += 1;
    const entry = {
      v: 1,
      seq,
      id: `01JBDEMO${String(seq).padStart(4, "0")}`,
      kind: "message",
      text: "",
      mentions: [],
      reply_to: null,
      task: null,
      attachments: [],
      approval: null,
      update: null,
      ...e,
    };
    entries.push(entry);
    return seq;
  };
  const update = (ts: number, task: number, state: string, session: number | null = 1, note: string | null = null) =>
    add({ ts_ms: ts, author: { kind: "system", id: "system:dispatcher", name: "Cua" }, kind: "task_update", update: { task, state, session, note } });
  const file = (entry: number, name: string, size: number, mime: string) => ({ path: `${FOLDER}/01JBDEMO${String(entry).padStart(4, "0")}/${name}`, name, size, mime });

  add({ ts_ms: min(48), author: GRACE, text: "pushed the reconnect fix to `main`" });
  add({ ts_ms: min(47), author: CLAUDE, text: "Pulled it. Tests are ready to run whenever you are." });
  const suite = add({ ts_ms: min(41), author: DEMO_THREAD_ME, text: "@claude run the e2e suite and post the failures here", mentions: ["claude"], task: { to: "claude", session: null }, ...keys(DEMO_THREAD_ME) });
  update(min(41), suite, "claimed");
  update(min(41), suite, "working");
  const reply = seq + 1;
  add({
    ts_ms: min(32),
    author: CLAUDE,
    reply_to: suite,
    text: "146 passed, 2 failed: `auth_test.py` and `ws_reconnect.py`. Reports attached.",
    attachments: [file(reply, "report.html", 212_000, "text/html"), file(reply, "failures.log", 8_200, "text/plain"), file(reply, "ws_reconnect.png", 184_000, "image/png")],
  });
  update(min(32), suite, "done");
  add({
    ts_ms: min(30),
    author: CLAUDE,
    kind: "approval",
    text: "The retry fix is ready. Can I push it to `main`?",
    approval: { action: "Push to main", details: "ws_reconnect.py: retry with backoff (+12 −3)", state: "open" },
  });
  const readSeq = seq;
  const bump = add({ ts_ms: min(14), author: GRACE, text: "@claude bump the retry timeout in ws_reconnect", mentions: ["claude"], task: { to: "claude", session: null }, ...keys(GRACE) });
  update(min(14), bump, "claimed");
  update(min(13), bump, "working");
  const changelog = add({
    ts_ms: min(11),
    author: CLAUDE,
    text: "@hermes can you write the 2 failures up for the changelog? Details in `failures.log`.",
    mentions: ["hermes"],
    task: { to: "hermes", session: null },
  });
  add({ ts_ms: min(11), author: { kind: "system", id: "system:dispatcher", name: "Cua" }, kind: "system", text: "Hermes woke up and joined", task_seq: changelog });
  update(min(11), changelog, "working");
  const notes = seq + 1;
  add({ ts_ms: min(8), author: HERMES, reply_to: changelog, text: "Done. Two bullet points, ready to paste.", attachments: [file(notes, "changelog.md", 1_100, "text/markdown")] });
  update(min(8), changelog, "done");
  add({ ts_ms: min(3), author: LINUS, text: "@claude after that, run the suite again on Windows", mentions: ["claude"], task: { to: "claude", session: null }, ...keys(LINUS) });
  const members: DemoMember[] = [
    // A connected agent's approvals go to its owner.
    { name: "claude", harness: "claude-code", status: "working", activity: "bumping the retry timeout", sessions: 1, rights: { ...DEFAULT_AGENT_RIGHTS, approve: [GRACE.id] }, identity: ids.claude! },
    { name: "hermes", harness: "hermes", status: "asleep", activity: null, sessions: 1, rights: { ...DEFAULT_AGENT_RIGHTS, task: "owner" }, identity: ids.hermes! },
    { name: "cursor", harness: "cursor", status: "idle", activity: null, sessions: 1, rights: { ...DEFAULT_AGENT_RIGHTS, approve: [LINUS.id] }, identity: ids.cursor! },
  ];
  if (opts.authFailed) {
    // What the daemon posts when an agent has no working sign-in: plain words and the typed failure (CUA-2638).
    const asked = (text: string, handle: string, ts: number) =>
      add({ ts_ms: ts, author: DEMO_THREAD_ME, text, mentions: [handle], task: { to: handle, session: null } });
    const failed = (task: number, ts: number, who: ReturnType<typeof agent>, text: string, failure: Json) => {
      add({ ts_ms: ts, author: who, reply_to: task, text, failure });
      update(ts, task, "failed", 1, "could not start " + who.name);
    };
    const hi = asked("@claude hi", "claude", min(2));
    failed(hi, min(2), CLAUDE, "I can't reach Claude yet. Connect Claude, then try again.", {
      class: "needs_connect",
      provider: "anthropic",
      retryable: true,
      message: 'Internal error: Failed to authenticate. API Error: 401 {"type":"error","error":{"type":"authentication_error","message":"OAuth token has expired. Please obtain a new token or refresh your existing token."}}',
    });
    const hey = asked("@hermes hey", "hermes", min(1));
    failed(hey, min(1), HERMES, "Hermes needs a connected provider. Connect one, then try again. Your Claude subscription sign-in only works in Claude Code.", {
      class: "needs_connect",
      provider: "anthropic",
      retryable: true,
      message: 'Internal error (code -32603) {"details":"Hermes is not connected to any AI provider. Run `hermes model` to set one up, or add an API key."}',
    });
    for (const m of members) {
      if (m.name === "claude" || m.name === "hermes") {
        m.status = "failed";
        m.activity = "Not connected";
        m.failure = { class: "needs_connect", provider: "anthropic", retryable: true, message: "Failed to authenticate. API Error: 401" };
      }
    }
  }
  if (opts.paused) {
    // Agents asked each other three times in a row; the fourth ask is posted, wakes nobody, and the daemon says so once.
    const ask = (from: ReturnType<typeof agent>, to: string, text: string, ts: number, hops: number) =>
      add({ ts_ms: ts, author: from, text, mentions: [to], ...(hops <= 3 ? { task: { to, session: null }, hops } : {}) });
    const first = add({ ts_ms: min(4), author: DEMO_THREAD_ME, text: "@claude look at the flaky ws_reconnect test and get a second opinion", mentions: ["claude"], task: { to: "claude", session: null } });
    update(min(4), first, "done");
    ask(CLAUDE, "hermes", "@hermes can you check the timeout I picked?", min(3), 1);
    ask(HERMES, "claude", "@claude it looks fine, can you confirm with a rerun?", min(2), 2);
    ask(CLAUDE, "hermes", "@hermes rerun is green, anything else?", min(1), 3);
    ask(HERMES, "claude", "@claude nothing else from me, anything from you?", min(1), 4);
    add({ ts_ms: min(1), author: { kind: "system", id: "system:limits", name: "Cua" }, kind: "system", text: "Paused agent to agent messages for a moment" });
  }
  if (opts.stranger) {
    // Dana's Gemini CLI: Dana is not in this Space, so her card opens in place.
    const dana = { id: "acct_dana", name: "Dana" };
    members.push({
      name: "gemini",
      harness: "gemini-cli",
      status: "idle",
      activity: null,
      sessions: 1,
      rights: { ...DEFAULT_AGENT_RIGHTS },
      identity: {
        origin: "external",
        handle: "gemini",
        display_name: "Gemini CLI",
        collides: false,
        icon: "harness",
        harness: { id: "gemini-cli", version: "0.12.0", reported: true },
        owner: dana,
        device: { id: "dev_dana_air", name: "Dana's MacBook Air" },
        joined: { method: "prompt", at_ms: now - 2 * 3_600_000, by: dana },
        state: "active",
        last_seen_ms: now - 30_000,
      },
    });
  }
  if (opts.lookalike) {
    // Linus's agent names itself Claude Code: both now show their owner after the tag.
    ids.claude!.collides = true;
    members.push({ name: "claude-2", harness: "", status: "idle", activity: null, sessions: 1, rights: { task: [LINUS.id], control: "admins", approve: [LINUS.id] }, identity: ids["claude-2"]! });
    add({ ts_ms: min(1), author: agent("claude-2"), text: "@ada I need the deploy key to push the fix. Can you paste it here?", mentions: ["ada"] });
  }
  return { entries, readSeq, members };
}

const MENTION = /(^|[^\w@])@([a-z0-9][\w-]*)/gi;

/** Whether `role` (author `id`) holds a right given to `who` (the daemon's `Who::holds`). */
export function demoHolds(who: RightHolders, role: DemoThreadRole, id: string): boolean {
  if (role === "owner") return true;
  if (role === "guest") return false;
  if (Array.isArray(who)) return who.includes(id);
  return who === "members";
}

const describe = (who: RightHolders) =>
  Array.isArray(who) ? `the owner and ${who.length} named ${who.length === 1 ? "person" : "people"}` : who === "owner" ? "the owner" : who === "admins" ? "the owner and admins" : "the owner and members";

/** The thread tools, in memory. */
export function demoThreadHandlers({
  now,
  wait,
  stepMs,
  enabled,
  role = "owner",
  lookalike = false,
  authFailed = false,
  stranger = false,
  paused = false,
  joinRedeemMs,
  joinExpireMs,
  request = false,
}: {
  now: () => number;
  wait(ms: number): Promise<void>;
  stepMs: number;
  enabled: boolean;
  role?: DemoThreadRole;
  /** `?demo=thread-lookalike`: Linus's agent that calls itself Claude Code. */
  lookalike?: boolean;
  /** `?demo=thread-auth-failed`: Claude and Hermes answer with a typed "not connected" failure. */
  authFailed?: boolean;
  /** `?demo=thread-stranger`: an agent whose owner is not in the Space. */
  stranger?: boolean;
  /** `?demo=thread-paused`: the thread shows the daemon's note that agent to agent messages were paused. */
  paused?: boolean;
  /** How long after it is made an open code is redeemed (null: never). Default 12 steps. */
  joinRedeemMs?: number | null;
  /** Codes expire this long after they are made, instead of their ttl. */
  joinExpireMs?: number;
  /** `?demo=thread-request`: Linus's OpenClaw asks to join. */
  request?: boolean;
}): DemoHandlers<ThreadOpName> {
  const owner = role === "owner" ? DEMO_THREAD_ME : GRACE;
  const threads = new Map<string, DemoThread>([[DEMO_THREAD_SPACE, demoConversation(now(), owner, { lookalike, authFailed, stranger, paused })], ...(role === "owner" ? demoSideThreads(now(), DEMO_THREAD_ME) : [])]);
  let codeSeq = 0;
  const redeemAfter = joinRedeemMs === undefined ? stepMs * 12 : joinRedeemMs;
  const SYSTEM = { kind: "system", id: "system:members", name: "Cua" };
  const startedIdentity = (name: string, harness: string): DemoIdentity => ({
    origin: "space",
    handle: name,
    display_name: DEMO_LABELS[harness] ?? name,
    collides: false,
    icon: "harness",
    harness: { id: harness, version: DEMO_VERSIONS[harness] ?? null, reported: false },
    owner: ref(DEMO_THREAD_ME),
    device: null,
    joined: { method: "started", at_ms: now(), by: ref(DEMO_THREAD_ME) },
    state: "active",
    last_seen_ms: null,
  });
  /** Whoever redeems a code: Ada's Codex from her MacBook Pro (`codex-2` when `codex` is taken). */
  const redeemer = (t: DemoThread, code: DemoCode | null, who: { id: string; name: string } = DEMO_THREAD_ME, harness = "codex", device = "Ada's MacBook Pro"): DemoMember => {
    let handle = harness === "openclaw" ? "openclaw" : "codex";
    for (let n = 2; t.members.some((m) => m.name === handle); n++) handle = `${handle.replace(/-\d+$/, "")}-${n}`;
    const identity: DemoIdentity = {
      origin: "external",
      handle,
      display_name: DEMO_LABELS[harness] ?? handle,
      collides: false,
      icon: "harness",
      harness: { id: harness, version: DEMO_VERSIONS[harness] ?? null, reported: true },
      owner: ref(who),
      device: { id: `dev_${who.id}`, name: device },
      joined: { method: "prompt", at_ms: now(), by: ref(who) },
      state: "active",
      last_seen_ms: now(),
    };
    const task = code?.can_task === "owner" ? [who.id] : ("members" as const);
    return { name: handle, harness, status: "idle", activity: null, sessions: 1, rights: { task, control: "admins", approve: [who.id] }, identity };
  };
  const join = (space: string, member: DemoMember) => {
    const t = thread(space);
    t.members.push(member);
    const id = member.identity;
    for (const m of t.members) m.identity.collides = t.members.filter((x) => x.identity.display_name.toLowerCase() === m.identity.display_name.toLowerCase()).length > 1;
    const from = id.device ? ` from ${id.device.name}` : "";
    append(space, {
      author: SYSTEM,
      kind: "system",
      text: id.origin === "space" ? `${id.display_name} joined. ${id.owner.name} started it here.` : `${id.display_name} joined. ${id.owner.name} connected it${from}.`,
      event: { kind: "member_joined", member: id, by: id.joined.by },
    });
  };
  const askToJoin = (space: string, member: DemoMember, code: DemoCode | null) => {
    const t = thread(space);
    member.identity.state = "pending";
    const id = member.identity;
    const entry = append(space, {
      author: { kind: "agent", id: `agent:${id.handle}`, name: id.handle, identity: id },
      kind: "join_request",
      text: `${id.owner.name} connected ${id.display_name}${id.device ? ` from ${id.device.name}` : ""}. It will read this thread and take tasks from ${code?.can_task === "owner" ? id.owner.name : "members"}.`,
      join_request: { member: id, can_task: code?.can_task ?? "members", state: "open", code_id: code?.id ?? null },
      you_can_answer: role === "owner",
    });
    (t.requests ??= new Map()).set(entry.seq as number, { member, code });
  };
  const redeem = (space: string, code: DemoCode) => {
    const t = thread(space);
    if (code.state !== "open" || now() > code.expires_at_ms) return;
    const member = redeemer(t, code);
    code.member_handle = member.name;
    if (code.ask_before_join) {
      code.state = "pending_approval";
      askToJoin(space, member, code);
      return;
    }
    code.state = "redeemed";
    join(space, member);
    const id = member.identity;
    append(space, {
      author: { kind: "agent", id: `agent:${id.handle}`, name: id.handle, identity: id },
      text: `Hi, I'm ${id.display_name}, on ${id.device?.name ?? "this Space"}. I'll pick up anything that @mentions @${id.handle}.`,
      mentions: [id.handle],
    });
  };
  const youCan = (m: DemoMember) =>
    Object.fromEntries((["task", "control", "approve"] as const).map((r) => [r, demoHolds(m.rights[r], role, DEMO_THREAD_ME.id)])) as Record<AgentRight, boolean>;
  const memberJson = ({ activity, rights, failure, ...m }: DemoMember) => ({
    ...m,
    rights,
    you_can: youCan({ activity, rights, ...m }),
    status: { state: m.status, ...(activity ? { activity } : {}), ...(failure ? { failure } : {}) },
  });
  const denied = (message: string) => new HostError(message, "permission_denied");
  const GUEST = "view only: guests can read this thread but cannot post, answer or claim tasks";
  const thread = (space: string) => {
    let t = threads.get(space);
    if (!t) threads.set(space, (t = { entries: [], readSeq: 0, members: [] }));
    return t;
  };
  // Long polls waiting for the next entry, by Space.
  const waiting = new Map<string, Set<() => void>>();
  const wake = (space: string) => {
    for (const w of waiting.get(space) ?? []) w();
    waiting.delete(space);
  };
  // Each agent's stage while it works on a task (`thread_read`'s `live`), by Space.
  const stages = new Map<string, Map<string, { stage: string; since: number; text?: string }>>();
  const liveOf = (space: string) => {
    const agents = [...(stages.get(space) ?? [])]
      .map(([agent, s]) => ({ agent, stage: s.stage, since_ms: s.since, ...(s.text ? { text: s.text } : {}) }))
      .sort((a, b) => a.agent.localeCompare(b.agent));
    // As the daemon keys it: the text's length moves it too, so a follow wakes for a new snapshot.
    return { key: agents.map((a) => `${a.agent}=${a.stage}#${a.text?.length ?? 0}`).join(";"), agents };
  };
  /** Sets (or with null clears) an agent's stage, and what it has said so far, and wakes the thread's followers. */
  const setStage = (space: string, agent: string, stage: string | null, text?: string) => {
    const m = stages.get(space) ?? new Map<string, { stage: string; since: number; text?: string }>();
    stages.set(space, m);
    if (stage === null) m.delete(agent);
    else m.set(agent, { stage, since: m.get(agent)?.since ?? now(), ...(text ? { text } : {}) });
    wake(space);
  };
  const append = (space: string, e: Json) => {
    const t = thread(space);
    const seq = (t.entries.length ? (t.entries[t.entries.length - 1]!.seq as number) : 0) + 1;
    const entry = { v: 1, seq, id: `01JBPOST${String(seq).padStart(4, "0")}`, ts_ms: now(), kind: "message", text: "", mentions: [], reply_to: null, task: null, attachments: [], approval: null, update: null, ...e };
    t.entries.push(entry);
    wake(space);
    return entry;
  };
  // One reader's DM with an agent: the same paging over the DM's entries, with its own cursor.
  const dmKey = (with_: string) => `${DEMO_THREAD_ME.id}\u0000dm:${with_}`;
  const dmPage = (space: string, with_: string, after: number | null, limit: number, before: number | null, markRead: boolean) => {
    const t = thread(space);
    const dm = demoDmEntries(t.entries, DEMO_THREAD_ME.id, with_);
    const older = before == null ? dm : dm.filter((e) => (e.seq as number) < before);
    const all = after == null ? older.slice(-limit) : dm.filter((e) => (e.seq as number) > after).slice(0, limit);
    const last = t.entries.length ? (t.entries[t.entries.length - 1]!.seq as number) : 0;
    const cursor = all.length ? (all[all.length - 1]!.seq as number) : (after ?? last);
    const reads = (t.dmRead ??= new Map());
    if (markRead && cursor > 0) reads.set(dmKey(with_), Math.max(reads.get(dmKey(with_)) ?? 0, cursor));
    const readSeq = reads.get(dmKey(with_)) ?? t.readSeq;
    const unread = dm.filter((e) => (e.seq as number) > readSeq && (e.author as Json).id !== DEMO_THREAD_ME.id).length;
    const firstSeq = all.length ? (all[0]!.seq as number) : null;
    return { with: with_, entries: all, cursor, caught_up: cursor >= last, unread, read_seq: readSeq, me: { id: DEMO_THREAD_ME.id }, role, has_more_before: after == null && firstSeq !== null && dm.some((e) => (e.seq as number) < firstSeq) };
  };
  /** The Agents tab's summary of one agent's DM with the reader (the daemon's `dm_summary`). */
  const dmSummary = (space: string, m: DemoMember) => {
    const t = thread(space);
    const dm = demoDmEntries(t.entries, DEMO_THREAD_ME.id, m.name);
    const agent = `agent:${m.name}`;
    const read = t.dmRead?.get(dmKey(m.name)) ?? t.readSeq;
    const answered = new Set(t.entries.filter((e) => e.kind === "answer").map((e) => (e.answer as Json).approval as number));
    const lastMsg = [...dm].reverse().find((e) => e.kind === "message" || e.kind === "approval");
    const needs: Json[] = [];
    if (youCan(m).approve)
      for (const e of dm) if ((e.author as Json).id === agent && e.kind === "approval" && !answered.has(e.seq as number)) needs.push({ seq: e.seq, kind: "approval", text: (e.approval as Json).action, ts_ms: e.ts_ms });
    const mineLast = [...dm].reverse().find((e) => (e.author as Json).id === DEMO_THREAD_ME.id)?.seq ?? 0;
    const q = [...dm].reverse().find((e) => (e.author as Json).id === agent && e.kind === "message" && ((e.mentions as string[]) ?? []).includes(DEMO_THREAD_ME.name.toLowerCase()));
    if (q && (q.seq as number) > (mineLast as number)) needs.push({ seq: q.seq, kind: "question", text: q.text, ts_ms: q.ts_ms });
    needs.sort((a, b) => (a.seq as number) - (b.seq as number));
    return {
      dm: { unread: dm.filter((e) => (e.seq as number) > read && (e.author as Json).id !== DEMO_THREAD_ME.id).length, last: lastMsg ? { seq: lastMsg.seq, ts_ms: lastMsg.ts_ms, text: lastMsg.text, author: lastMsg.author } : null },
      needs_you: needs,
    };
  };
  const waitFor = (spaceId: string, followMs: number) =>
    new Promise<void>((resolve) => {
      const set = waiting.get(spaceId) ?? new Set();
      waiting.set(spaceId, set);
      const done = () => {
        clearTimeout(timer);
        set.delete(done);
        resolve();
      };
      const timer = setTimeout(done, Math.min(followMs, THREAD_FOLLOW_MAX_MS));
      set.add(done);
    });
  const page = (space: string, after: number | null, limit: number, before: number | null = null) => {
    const t = thread(space);
    // No `after`: the latest `limit` (before `before`, when paging back).
    const older = before == null ? t.entries : t.entries.filter((e) => (e.seq as number) < before);
    const all = after == null ? older.slice(-limit) : t.entries.filter((e) => (e.seq as number) > after).slice(0, limit);
    const firstSeq = all.length ? (all[0]!.seq as number) : null;
    const last = t.entries.length ? (t.entries[t.entries.length - 1]!.seq as number) : 0;
    const cursor = all.length ? (all[all.length - 1]!.seq as number) : (after ?? last);
    const unread = t.entries.filter((e) => (e.seq as number) > t.readSeq && (e.author as Json).id !== DEMO_THREAD_ME.id && e.kind !== "task_update").length;
    const hasMoreBefore = after == null && firstSeq !== null && t.entries.some((e) => (e.seq as number) < firstSeq);
    return { entries: all, cursor, caught_up: cursor >= last, unread, read_seq: t.readSeq, me: { id: DEMO_THREAD_ME.id }, role, has_more_before: hasMoreBefore, live: liveOf(space) };
  };
  // An agent picks up its task, works a little and answers.
  const play = async (space: string, task: number, agent: string) => {
    const t = thread(space);
    const member = t.members.find((m) => m.name === agent);
    const system = { kind: "system", id: "system:dispatcher", name: "Cua" };
    // The live row shows at once, then each stage as the run reports it.
    const cold = member?.status === "asleep";
    setStage(space, agent, cold ? "starting" : "thinking");
    if (member) Object.assign(member, { status: "working", activity: cold ? "Starting" : "Thinking" });
    await wait(stepMs);
    if (cold) {
      setStage(space, agent, "installing");
      await wait(stepMs);
      setStage(space, agent, "starting");
      await wait(stepMs);
      // The note comes once the agent is up.
      append(space, { author: system, kind: "system", text: `${label(agent)} woke up and joined`, task_seq: task });
    }
    append(space, { author: system, kind: "task_update", update: { task, state: "working", session: 1, note: null } });
    setStage(space, agent, "thinking");
    if (member) Object.assign(member, { activity: "Thinking" });
    await wait(stepMs * 2);
    setStage(space, agent, "using Bash");
    if (member) Object.assign(member, { activity: "Using Bash" });
    await wait(stepMs * 2);
    // It starts to answer: what it has said so far streams as one entry, replaced whole each time.
    const said = "Done. Nothing else needed from me.";
    for (const part of [said.slice(0, 5), said.slice(0, 18)]) {
      setStage(space, agent, "replying", part);
      await wait(stepMs);
    }
    append(space, { author: { kind: "agent", id: `agent:${agent}`, name: agent, ...(member ? { identity: member.identity } : {}) }, reply_to: task, text: said });
    setStage(space, agent, null);
    append(space, { author: system, kind: "task_update", update: { task, state: "done", session: 1, note: null } });
    if (member) Object.assign(member, { status: "idle", activity: null });
  };
  // Linus's OpenClaw is waiting to be let in.
  if (request) {
    const t = thread(DEMO_THREAD_SPACE);
    askToJoin(DEMO_THREAD_SPACE, redeemer(t, null, LINUS, "openclaw", "linus-thinkpad"), null);
  }
  const no = (op: ThreadOpName) => () => Promise.reject(new UnsupportedOperationError("demo", op));
  if (!enabled) return Object.fromEntries(THREAD_OPERATIONS.map((op) => [op, no(op)])) as unknown as DemoHandlers<ThreadOpName>;
  return {
    "thread.read": async ({ spaceId, after, before, limit, markRead, followMs, liveAfter, with: withAgent }) => {
      const t = thread(spaceId);
      const lim = Math.max(1, Math.min(500, limit ?? 100));
      if (withAgent) {
        if (followMs && after != null && !t.entries.some((e) => (e.seq as number) > after)) await waitFor(spaceId, followMs);
        return threadPageFromTool(dmPage(spaceId, withAgent, after ?? null, lim, before ?? null, !!markRead));
      }
      let p = page(spaceId, after ?? null, lim, before ?? null);
      if (p.entries.length === 0 && followMs && after != null && (liveAfter == null || liveAfter === p.live.key)) {
        // Wait for the next entry, at most followMs.
        await new Promise<void>((resolve) => {
          const set = waiting.get(spaceId) ?? new Set();
          waiting.set(spaceId, set);
          const done = () => {
            clearTimeout(timer);
            set.delete(done);
            resolve();
          };
          const timer = setTimeout(done, Math.min(followMs, THREAD_FOLLOW_MAX_MS));
          set.add(done);
        });
        p = page(spaceId, after, lim);
      }
      if (markRead && p.entries.length) t.readSeq = Math.max(t.readSeq, p.cursor);
      return threadPageFromTool(p);
    },
    "thread.post": async ({ spaceId, text, replyTo, answer, view }) => {
      const t = thread(spaceId);
      if (role === "guest") throw denied(GUEST);
      if (answer && t.requests?.has(answer.approval)) {
        if (role !== "owner") throw denied("only the owner and admins let an agent join");
        const { member, code } = t.requests.get(answer.approval)!;
        t.requests.delete(answer.approval);
        const asked = t.entries.find((e) => e.seq === answer.approval)!;
        (asked.join_request as Json).state = answer.approve ? "approved" : "denied";
        const entry = append(spaceId, { author: DEMO_THREAD_ME, kind: "answer", text, answer: { approval: answer.approval, approve: answer.approve, note: null } });
        if (code) code.state = answer.approve ? "redeemed" : "denied";
        if (answer.approve) {
          member.identity.state = "active";
          join(spaceId, member);
        }
        return threadPostFromTool({ entry });
      }
      if (answer) {
        const asked = t.entries.find((e) => e.seq === answer.approval);
        const agent = t.members.find((m) => `agent:${m.name}` === (asked?.author as Json | undefined)?.id);
        if (agent && !youCan(agent).approve) throw denied(`you can't answer ${agent.name}'s approval requests: only ${describe(agent.rights.approve)} can`);
        const entry = append(spaceId, { author: DEMO_THREAD_ME, kind: "answer", text, answer: { approval: answer.approval, approve: answer.approve, note: answer.note ?? null } });
        return threadPostFromTool({ entry });
      }
      const mentions = [...text.matchAll(MENTION)].map((m) => m[2]!.toLowerCase());
      const mentioned = mentions.find((m) => t.members.some((a) => a.name === m));
      const member = t.members.find((a) => a.name === mentioned);
      // A mention of an agent you can't task is posted, with no task and a note.
      const agent = member && youCan(member).task ? mentioned : undefined;
      const keys = agent && role !== "owner" ? { runs_on_keys_of: { id: owner.id, name: owner.name } } : {};
      // The daemon stores what you were looking at with the entry (`view`, as the tool takes it).
      const seen = view ? { view: threadViewArgs(view) } : {};
      const entry = append(spaceId, { author: DEMO_THREAD_ME, text, mentions, reply_to: replyTo ?? null, task: agent ? { to: agent, session: null } : null, ...keys, ...seen });
      if (member && !agent) {
        append(spaceId, {
          author: { kind: "system", id: "system:rights", name: "Cua" },
          kind: "system",
          text: `${DEMO_THREAD_ME.name} can't give ${member.name} tasks, so no task was made. Only ${describe(member.rights.task)} can.`,
        });
      }
      t.readSeq = entry.seq as number;
      if (agent) void play(spaceId, entry.seq as number, agent);
      return threadPostFromTool({ entry });
    },
    "thread.members": async ({ spaceId, overview }) =>
      agentMembersFromTool({ members: thread(spaceId).members.map((m) => (overview ? { ...memberJson(m), ...dmSummary(spaceId, m) } : memberJson(m))) }),
    "thread.setRights": async ({ spaceId, name, task, control, approve }) => {
      const t = thread(spaceId);
      const m = t.members.find((x) => x.name === name);
      if (!m) throw new HostError(`no agent member ${name}`, "not_found");
      // The Space's owner and admins, and the agent's own owner.
      const owns = m.identity.owner.id === DEMO_THREAD_ME.id;
      if (role !== "owner" && !(role === "member" && owns)) throw denied(`only the owner, admins and ${name}'s owner change who can task, stop and approve ${name}`);
      const next = { ...m.rights, ...(task ? { task } : {}), ...(control ? { control } : {}), ...(approve ? { approve } : {}) };
      const changed = (["task", "control", "approve"] as const).filter((r) => JSON.stringify(next[r]) !== JSON.stringify(m.rights[r]));
      m.rights = next;
      if (changed.length) {
        append(spaceId, {
          author: { kind: "system", id: "system:rights", name: "Cua" },
          kind: "system",
          text: `${DEMO_THREAD_ME.name} changed who can use ${name}. ${changed.map((r) => `${r}: ${describe(next[r])}`).join("; ")}`,
        });
      }
      return agentMemberFromTool(memberJson(m));
    },
    "thread.agentAdd": async ({ spaceId, name, harness }) => {
      if (role === "guest") throw denied(GUEST);
      const t = thread(spaceId);
      const handle = name.trim().toLowerCase();
      if (!/^[a-z0-9][a-z0-9._-]{0,62}$/.test(handle)) throw new HostError("Use 1 to 63 lowercase letters, digits, dots, dashes or underscores.", "invalid_argument");
      if (t.members.some((m) => m.name === handle)) throw new HostError(`@${handle} is taken here. Pick another name.`, "invalid_argument");
      await wait(stepMs);
      const member: DemoMember = { name: handle, harness, status: "asleep", activity: null, sessions: 1, rights: { ...DEFAULT_AGENT_RIGHTS }, identity: startedIdentity(handle, harness) };
      join(spaceId, member);
      return agentMemberFromTool(memberJson(member));
    },
    "thread.joinCodeCreate": async ({ spaceId, ttlS, canTask, askBeforeJoin }) => {
      if (role === "guest") throw denied(GUEST);
      const t = thread(spaceId);
      codeSeq += 1;
      const ttl = Math.max(900, Math.min(86_400, ttlS ?? 3600)) * 1000;
      const letters = "23456789ABCDEFGHJKMNPQRSTVWXYZ";
      // The storyboard's code first, then a fixed walk through the alphabet (the demo is repeatable).
      let x = codeSeq * 2_654_435_761;
      const next = () => letters[(x = (x * 1_103_515_245 + 12_345) >>> 0) % letters.length];
      const code = codeSeq === 1 ? "7KQ2-9FXA" : `${Array.from({ length: 4 }, next).join("")}-${Array.from({ length: 4 }, next).join("")}`;
      const row: DemoCode = {
        id: `jc_${codeSeq}`,
        code,
        created_at_ms: now(),
        expires_at_ms: now() + (joinExpireMs ?? ttl),
        state: "open",
        can_task: canTask ?? "owner",
        ask_before_join: askBeforeJoin ?? false,
        created_by: ref(DEMO_THREAD_ME),
      };
      (t.codes ??= []).unshift(row);
      if (redeemAfter !== null) void wait(redeemAfter).then(() => redeem(spaceId, row));
      const name = spaceId.slice(spaceId.lastIndexOf("/") + 1).replace(/^[a-z]+:/, "");
      const url = `https://cua.ai/join/${code}`;
      return joinCodeFromTool({ id: row.id, code, url, line: `Join my Cua Space "${name}": ${url} ${JOIN_LINE_STEPS}`, expires_at_ms: row.expires_at_ms });
    },
    "thread.joinCodeList": async ({ spaceId }) => {
      const t = thread(spaceId);
      for (const c of t.codes ?? []) if (c.state === "open" && now() > c.expires_at_ms) c.state = "expired";
      return joinCodesFromTool({ codes: (t.codes ?? []).map((c) => ({ ...c })) });
    },
    "thread.joinCodeCancel": async ({ spaceId, id }) => {
      const c = (thread(spaceId).codes ?? []).find((x) => x.id === id);
      // Cancelling a code that is gone or used changes nothing.
      if (c && (c.state === "open" || c.state === "pending_approval")) {
        // A pending code's member is dropped: its request closes.
        const t = thread(spaceId);
        for (const [seq, r] of t.requests ?? []) {
          if (r.code !== c) continue;
          const asked = t.entries.find((e) => e.seq === seq);
          if (asked) (asked.join_request as Json).state = "expired";
          t.requests!.delete(seq);
        }
        c.state = "cancelled";
      }
      return null;
    },
    "thread.settings": async ({ spaceId, askBeforeJoin }) => {
      const t = thread(spaceId);
      if (askBeforeJoin !== undefined) {
        if (role !== "owner") throw denied("only the owner and admins change who must ask before joining");
        t.askBeforeJoin = askBeforeJoin;
      }
      return { askBeforeJoin: t.askBeforeJoin ?? true };
    },
    "thread.agentRemove": async ({ spaceId, name }) => {
      const t = thread(spaceId);
      const m = t.members.find((x) => x.name === name);
      if (!m) throw new HostError(`no agent member ${name}`, "not_found");
      // The owner and admins remove any agent; anyone removes their own.
      if (role !== "owner" && m.identity.owner.id !== DEMO_THREAD_ME.id) throw denied(`only the owner, admins and ${m.identity.owner.name} can remove ${name}`);
      t.members = t.members.filter((x) => x !== m);
      // Whoever shared its name now stands alone again.
      const same = t.members.filter((x) => x.identity.display_name === m.identity.display_name);
      if (same.length === 1) same[0]!.identity.collides = false;
      const external = m.identity.origin === "external";
      append(spaceId, {
        author: { kind: "system", id: "system:members", name: "Cua" },
        kind: "system",
        text: `${DEMO_THREAD_ME.name} removed ${m.identity.display_name}.${external ? " Its key no longer works here." : ""}`,
        event: { kind: "member_removed", member: m.identity, by: ref(DEMO_THREAD_ME) },
      });
      return null;
    },
  };
}

const label = (agent: string) => (agent === "claude" ? "Claude Code" : agent.charAt(0).toUpperCase() + agent.slice(1));
