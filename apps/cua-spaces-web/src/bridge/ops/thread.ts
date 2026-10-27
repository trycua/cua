// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Space thread (CUA-2155): one append-only log per Space, its agent
 * members, and posting to it. Each operation is one daemon Spaces tool
 * (DESIGN §4.1), called through the host's existing tool path
 * (`callToolJson`), so a remote Space's owner answers over the relay as for
 * every other tool.
 *
 * | Operation | Daemon tool | SwiftUI / Electron host | Tauri |
 * |---|---|---|---|
 * | `thread.read {spaceId, after?, limit?, markRead?, followMs?}` | `thread_read` (`follow_ms` long-polls) | the tool's JSON as-is | unsupported |
 * | `thread.post {spaceId, text, replyTo?, answer?, toSession?, view?}` | `thread_post` | the tool's JSON as-is | unsupported |
 * | `thread.members {spaceId}` | `agent_members` | the tool's JSON as-is | unsupported |
 * | `thread.setRights {spaceId, name, task?, control?, approve?}` | `agent_set_rights` | the tool's JSON as-is | unsupported |
 * | `thread.agentAdd {spaceId, name, harness}` | `agent_add` | the tool's JSON as-is | unsupported |
 * | `thread.agentRemove {spaceId, name}` | `agent_remove` | the tool's JSON as-is | unsupported |
 * | `thread.joinCodeCreate {spaceId, ttlS?, canTask?, askBeforeJoin?}` | `thread_join_code_create` | the tool's JSON as-is | unsupported |
 * | `thread.joinCodeList {spaceId}` | `thread_join_code_list` | the tool's JSON as-is | unsupported |
 * | `thread.joinCodeCancel {spaceId, id}` | `thread_join_code_cancel` | the tool's JSON as-is | unsupported |
 * | `thread.settings {spaceId, askBeforeJoin?}` | `thread_settings` | the tool's JSON as-is | unsupported |
 *
 * The native hosts stay thin: they turn the operation's arguments into the
 * tool's (`threadReadArgs`, ...) and answer the tool's JSON untouched; this
 * file maps it (`threadPageFromTool`, ...) for every host, so the mapping
 * lives once. A daemon without the thread (no `thread_*` tools: the SDK's
 * or the daemon's "not found") answers `unsupported`, and the Space page
 * keeps its old layout.
 */

import { HostError, isUnsupported, UnsupportedOperationError, type BridgeMode } from "../adapter";
import type { OpCoverage } from "../coverage";

/* ---- The thread as the page reads it (camelCase) ---------------------------------- */

export type ThreadAuthorKind = "person" | "agent" | "system";

/** Who wrote an entry, stamped by the owner daemon from the caller:
 * a relay account id (`owner` on a machine that is not signed in),
 * `agent:<name>`, or `system:<what>`. */
export interface ThreadAuthor {
  kind: ThreadAuthorKind;
  id: string;
  name: string;
  /** An agent's identity when it posted (CUA-2262): its posts keep their mark and owner after it leaves. */
  identity?: MemberIdentity | null;
}

/* ---- Who an agent member is (CUA-2262) ----------------------------------------------- */

export interface MemberRef {
  id: string;
  name: string;
}

/** How an agent member came into the Space. */
export type JoinMethod = "started" | "prompt" | "setup";

/**
 * An agent member's identity. The agent sets its name, its icon and the
 * harness it reports; the owner, the device and how it joined come from the
 * member's record (who added it, the computer whose key redeemed the code),
 * never from the agent, so the app can mark them as checked by Cua.
 */
export interface MemberIdentity {
  /** `space`: started inside the Space; `external`: connected from a computer with `cua join`. */
  origin: "space" | "external";
  /** The unique `@handle` in the Space (the member's name). */
  handle: string;
  /** What the agent calls itself; two members may share it. */
  displayName: string;
  /** Another member uses the same name: the owner shows after the AI tag. */
  collides: boolean;
  /** The agent's own icon: null (its harness's mark), `harness`, or an https URL. It replaces the tile only. */
  icon: string | null;
  /** The harness as the agent reported it (`reported: false`: it didn't say). */
  harness: { id: string; version: string | null; reported: boolean };
  owner: MemberRef;
  /** The computer it posts from (external agents). Never an address or a place. */
  device: MemberRef | null;
  joined: { method: JoinMethod; atMs: number | null; by: MemberRef | null };
  /** `pending`: it redeemed a code and waits for the owner or an admin to let it in. */
  state: "active" | "pending";
  lastSeenMs: number | null;
}

/** A join, a removal, a refused name or a rate limit, on a system note (CUA-2262/2264). */
export interface MemberEvent {
  kind: "member_joined" | "member_removed" | "name_refused" | "rate_limited";
  member: MemberIdentity | null;
  by: MemberRef | null;
}

export type ThreadEntryKind = "message" | "task_update" | "approval" | "answer" | "system" | "join_request";

/** An agent asks to join (CUA-2263): the owner or an admin answers it like an approval. */
export interface JoinRequest {
  member: MemberIdentity | null;
  /** Who can give it tasks once it is in. */
  canTask: "owner" | "members";
  /** As posted (`open`); the answer is a later `answer` entry. */
  state: string;
  codeId: string;
  /** The caller may let it in (the owner and admins, while it is open). */
  youCanAnswer: boolean;
}

/** A task's state, folded from its `task_update` entries. */
export type TaskState = "queued" | "claimed" | "working" | "done" | "failed";

/** A Volume reference: never inline bytes, never a host path. */
export interface ThreadAttachment {
  path: string;
  name: string;
  size: number | null;
  mime: string | null;
}

/** What failed in an agent's turn or start, typed (`cua_agents::failure::Failure`). */
export interface ThreadFailure {
  /** `needs_connect`: no working credential, connect (or reconnect) one; `error`: anything else. */
  class: "needs_connect" | "error";
  /** For `needs_connect`: the Connectors row that fixes it (`anthropic`, `openai`). */
  provider: string | null;
  /** The complete error as the harness or provider said it (for Show details; never the summary). */
  message: string;
  /** Sending the same message again can work. */
  retryable: boolean;
}

export interface ThreadEntry {
  seq: number;
  id: string;
  tsMs: number;
  author: ThreadAuthor;
  kind: ThreadEntryKind;
  text: string;
  /** Member names mentioned with `@name`. */
  mentions: string[];
  replyTo: number | null;
  /** A message that @mentions an agent member is a task for it. */
  task: { to: string; session: number | null } | null;
  attachments: ThreadAttachment[];
  /** An agent asks before it acts. */
  approval: { action: string; details: string; state: string } | null;
  /** Someone answered an approval (`kind: "answer"`). */
  answer: { approval: number; approve: boolean; note: string | null } | null;
  /** A system note about a task (`task_seq` on a `system` entry): it shows inside that task. */
  noteFor: number | null;
  /** A task moved on (`kind: "task_update"`). */
  update: { task: number; state: TaskState; session: number | null; note: string | null } | null;
  /** A task someone other than the owner gave, run on the owner's provider keys. */
  runsOnKeysOf: { id: string; name: string } | null;
  /** A system note about a member joining, leaving or a refused name (CUA-2262). */
  event: MemberEvent | null;
  /** On a `join_request` entry (CUA-2263). */
  joinRequest?: JoinRequest | null;
  /** What the person was looking at in the Space as they posted (`view`): its desktop or a window. */
  view?: ThreadView | null;
  /** On an agent's reply to a task that failed: what failed, with the complete text. `text` says it in plain words. */
  failure?: ThreadFailure | null;
}

/** `thread_post`'s `view` and every entry's: the Space's desktop, or one of its windows (`cua.env.v1.ThreadView`). */
export interface ThreadView {
  kind: "desktop" | "window";
  app: string | null;
  title: string | null;
  windowId: string | null;
}

/** Your role in the Space, as the owner daemon sees you: a guest reads only. */
export type ThreadRole = "owner" | "admin" | "member" | "guest" | "agent";

export interface ThreadPage {
  entries: ThreadEntry[];
  /** The last seq this page covers: the next read's `after`. */
  cursor: number;
  caughtUp: boolean;
  /** Entries after the caller's read cursor. */
  unread: number;
  /** The caller's read cursor, when the daemon says it. */
  readSeq: number | null;
  /** The caller's own author id, when the daemon says it (the page's "me"). */
  me: string | null;
  /** Older entries exist before this page (read them with `before`). */
  hasMoreBefore: boolean;
  /** Your role here, when the daemon says it (older daemons: null, read as the owner's). */
  role: ThreadRole | null;
}

/** Who holds one of an agent's rights: a group, or these account ids (besides the owner). */
export type RightHolders = "owner" | "admins" | "members" | string[];

export type AgentRight = "task" | "control" | "approve";

/** Who can give an agent tasks, control it and answer its approvals. */
export type AgentRights = Record<AgentRight, RightHolders>;

/** `working`, `idle`, `waiting`, `paused` or `asleep` (other words pass through). */
export type AgentMemberStatus = string;

export interface AgentMember {
  name: string;
  /** What the Space calls it (`@claude`) when that is not `name` (a second Space's `claude` is stored as `claude-2`). */
  handle?: string;
  /** Harness id (`claude-code`, `hermes`, ...). */
  harness: string;
  status: AgentMemberStatus;
  /** What it is doing, in a few words ("running e2e suite"), when it says. */
  activity: string | null;
  /** For a failed member: what failed, typed, with the complete text. */
  failure?: ThreadFailure | null;
  /** Sessions it runs (P2 shows each). */
  sessions: number;
  /** Its rights (null from a daemon without them: the defaults apply). */
  rights: AgentRights | null;
  /** What you may do with it (null from a daemon without rights: everything). */
  youCan: Record<AgentRight, boolean> | null;
  /** Who took control of it, while someone has. */
  heldBy: string | null;
  /** Who it is: owner, device, how it joined (null from a daemon without it: no owner badge, no card). */
  identity: MemberIdentity | null;
  /** Its DM with you (`overview`, CUA-2266); null when not asked or not said. */
  dm?: AgentDm | null;
  /** What waits for you from it (`overview`): open approvals you may answer, its question to you. */
  needsYou?: NeedsYou[];
}

export interface ThreadMembersArgs {
  spaceId: string;
  /** Each agent's DM with you as well (`dm`, `needsYou`). */
  overview?: boolean;
}

/** One agent's DM with you, summed up for the Agents tab. */
export interface AgentDm {
  unread: number;
  last: { seq: number; tsMs: number; text: string; author: ThreadAuthor } | null;
}

export interface NeedsYou {
  seq: number;
  kind: "approval" | "question";
  text: string;
  tsMs: number;
}

/** The defaults a daemon applies when an agent has none set. */
export const DEFAULT_AGENT_RIGHTS: AgentRights = { task: "members", control: "admins", approve: "admins" };

export interface ThreadReadArgs {
  spaceId: string;
  after?: number | null;
  /** Page back: the entries before this seq (the latest `limit` of them). */
  before?: number | null;
  limit?: number | null;
  markRead?: boolean;
  /** Long-poll up to this long (0..25000 ms) for entries after `after`. */
  followMs?: number;
  /** Only your DM with this agent member (its handle, CUA-2266): paging, `unread` and `markRead` apply to the DM. */
  with?: string | null;
}

export interface ThreadPostArgs {
  spaceId: string;
  text: string;
  replyTo?: number | null;
  /** Answer an approval entry. */
  answer?: { approval: number; approve: boolean; note?: string | null } | null;
  /** P2: one session of an agent instead of any free one. */
  toSession?: number | null;
  /** What the person is looking at in the Space as they post (the main view's window), stored with the entry. */
  view?: ThreadView | null;
}

export interface ThreadSetRightsArgs {
  spaceId: string;
  /** The agent member. */
  name: string;
  task?: RightHolders;
  control?: RightHolders;
  approve?: RightHolders;
}

export interface ThreadAgentRemoveArgs {
  spaceId: string;
  /** The agent member's handle. */
  name: string;
}

export interface ThreadAgentAddArgs {
  spaceId: string;
  /** The new member's handle (`a-z`, `0-9`, `.`, `_`, `-`). */
  name: string;
  /** The harness id (`hermes`, `claude-code`, `openai-codex`, ...). */
  harness: string;
}

export interface ThreadJoinCodeCreateArgs {
  spaceId: string;
  /** Seconds until it expires: 900 to 86400 (default 3600). */
  ttlS?: number;
  /** Who can task the agent: whoever made the code, or members. */
  canTask?: "owner" | "members";
  /** Ask the owner or an admin before it joins. */
  askBeforeJoin?: boolean;
}

/** A new join code: its plaintext is only ever here. */
export interface JoinCode {
  id: string;
  code: string;
  url: string;
  /** The one line to paste into an agent. */
  line: string;
  expiresAtMs: number;
}

export type JoinCodeState = "open" | "pending_approval" | "redeemed" | "denied" | "expired" | "cancelled";

export interface JoinCodeRow {
  id: string;
  createdAtMs: number;
  expiresAtMs: number;
  state: JoinCodeState;
  canTask: "owner" | "members";
  askBeforeJoin: boolean;
  createdBy: MemberRef | null;
  /** The member it made, once used. */
  memberHandle: string | null;
}

export interface ThreadSettings {
  /** "Ask before members' agents join". */
  askBeforeJoin: boolean;
}

export interface ThreadOps {
  "thread.read": { args: ThreadReadArgs; result: ThreadPage };
  "thread.post": { args: ThreadPostArgs; result: ThreadEntry | null };
  "thread.members": { args: ThreadMembersArgs; result: { members: AgentMember[] } };
  "thread.setRights": { args: ThreadSetRightsArgs; result: AgentMember | null };
  /** Starts an agent in the Space (it sleeps until someone @mentions it). */
  "thread.agentAdd": { args: ThreadAgentAddArgs; result: AgentMember | null };
  /** Removes an agent member; a connected agent's key stops at once. */
  "thread.agentRemove": { args: ThreadAgentRemoveArgs; result: null };
  "thread.joinCodeCreate": { args: ThreadJoinCodeCreateArgs; result: JoinCode };
  "thread.joinCodeList": { args: { spaceId: string }; result: { codes: JoinCodeRow[] } };
  "thread.joinCodeCancel": { args: { spaceId: string; id: string }; result: null };
  /** Reads, or with `askBeforeJoin` sets, the Space's thread settings. */
  "thread.settings": { args: { spaceId: string; askBeforeJoin?: boolean }; result: ThreadSettings };
}

export type ThreadOpName = keyof ThreadOps;

export const THREAD_OPERATIONS = [
  "thread.read",
  "thread.post",
  "thread.members",
  "thread.setRights",
  "thread.agentAdd",
  "thread.agentRemove",
  "thread.joinCodeCreate",
  "thread.joinCodeList",
  "thread.joinCodeCancel",
  "thread.settings",
] as const satisfies readonly ThreadOpName[];

/** Operations the native hosts don't route yet (none: CUA-2263 routes every one). */
export const THREAD_PENDING_OPERATIONS = [] as const satisfies readonly ThreadOpName[];
const pending = (op: ThreadOpName): boolean => (THREAD_PENDING_OPERATIONS as readonly ThreadOpName[]).includes(op);

/** Whether `mode`'s host answers `op` today: the page hides what it can't do rather than show a button that fails. */
export function threadOpAvailable(mode: BridgeMode, op: ThreadOpName): boolean {
  if (mode === "tauri") return false;
  return mode === "demo" || !pending(op);
}

/** What the native hosts route today. */
export type ThreadWebkitMethod = Exclude<ThreadOpName, (typeof THREAD_PENDING_OPERATIONS)[number]>;

/** The SwiftUI host and the Electron shell route each under its own name. */
export const THREAD_WEBKIT_METHODS = THREAD_OPERATIONS.filter((op) => !pending(op)) as ThreadWebkitMethod[];

const TAURI_WHY = "the Space thread is the cua daemon's; the Tauri shell's agents_tool does not allow it";
export const THREAD_COVERAGE = {
  "thread.read": { webkit: { methods: ["thread.read"] }, tauri: [], unsupported: { tauri: TAURI_WHY } },
  "thread.post": { webkit: { methods: ["thread.post"] }, tauri: [], unsupported: { tauri: TAURI_WHY } },
  "thread.members": { webkit: { methods: ["thread.members"] }, tauri: [], unsupported: { tauri: TAURI_WHY } },
  "thread.setRights": { webkit: { methods: ["thread.setRights"] }, tauri: [], unsupported: { tauri: TAURI_WHY } },
  "thread.agentAdd": { webkit: { methods: ["thread.agentAdd"] }, tauri: [], unsupported: { tauri: TAURI_WHY } },
  "thread.agentRemove": { webkit: { methods: ["thread.agentRemove"] }, tauri: [], unsupported: { tauri: TAURI_WHY } },
  "thread.joinCodeCreate": { webkit: { methods: ["thread.joinCodeCreate"] }, tauri: [], unsupported: { tauri: TAURI_WHY } },
  "thread.joinCodeList": { webkit: { methods: ["thread.joinCodeList"] }, tauri: [], unsupported: { tauri: TAURI_WHY } },
  "thread.joinCodeCancel": { webkit: { methods: ["thread.joinCodeCancel"] }, tauri: [], unsupported: { tauri: TAURI_WHY } },
  "thread.settings": { webkit: { methods: ["thread.settings"] }, tauri: [], unsupported: { tauri: TAURI_WHY } },
} as const satisfies Record<ThreadOpName, OpCoverage>;

/** Arguments that take each operation down its usual path (the coverage and contract tests). */
export const THREAD_ARGS: { [K in ThreadOpName]: ThreadOps[K]["args"] } = {
  "thread.read": { spaceId: "local:design-review", after: 0, limit: 100 },
  "thread.post": { spaceId: "local:design-review", text: "@claude run the e2e suite" },
  "thread.members": { spaceId: "local:design-review" },
  "thread.setRights": { spaceId: "local:design-review", name: "claude", task: "owner" },
  "thread.agentAdd": { spaceId: "local:design-review", name: "hermes", harness: "hermes" },
  "thread.agentRemove": { spaceId: "local:design-review", name: "claude-2" },
  "thread.joinCodeCreate": { spaceId: "local:design-review", ttlS: 3600, canTask: "owner", askBeforeJoin: false },
  "thread.joinCodeList": { spaceId: "local:design-review" },
  "thread.joinCodeCancel": { spaceId: "local:design-review", id: "jc_0123456789ab" },
  "thread.settings": { spaceId: "local:design-review" },
};

/** The long poll's longest wait (`thread_read`'s `follow_ms` bound). */
export const THREAD_FOLLOW_MAX_MS = 25_000;

/* ---- Operation arguments -> tool arguments ------------------------------------------ */

type Json = Record<string, unknown>;

export function threadReadArgs(a: ThreadReadArgs): Json {
  const out: Json = { space: a.spaceId };
  if (a.after != null) out.after = a.after;
  if (a.before != null) out.before = a.before;
  if (a.limit != null) out.limit = a.limit;
  if (a.markRead) out.mark_read = true;
  if (a.followMs) out.follow_ms = Math.max(0, Math.min(THREAD_FOLLOW_MAX_MS, Math.round(a.followMs)));
  if (a.with) out.with = a.with;
  return out;
}

export function threadPostArgs(a: ThreadPostArgs): Json {
  const out: Json = { space: a.spaceId, text: a.text };
  if (a.replyTo != null) out.reply_to = a.replyTo;
  if (a.answer) out.answer = { approval: a.answer.approval, approve: a.answer.approve, ...(a.answer.note ? { note: a.answer.note } : {}) };
  if (a.toSession != null) out.to_session = a.toSession;
  if (a.view) out.view = threadViewArgs(a.view);
  return out;
}

/** `view` as the tool takes it (snake_case, only the fields there are). */
export function threadViewArgs(v: ThreadView): Json {
  const out: Json = { kind: v.kind };
  if (v.app) out.app = v.app;
  if (v.title) out.title = v.title;
  if (v.windowId) out.window_id = v.windowId;
  return out;
}

/** An entry's `view`, as the tool gives it; null without one. */
export function threadViewFromTool(v: unknown): ThreadView | null {
  const o = obj(v);
  if (!o || (o.kind !== "desktop" && o.kind !== "window")) return null;
  return { kind: o.kind, app: str(o.app) || null, title: str(o.title) || null, windowId: str(o.window_id) || null };
}

export const threadMembersArgs = (a: { spaceId: string }): Json => ({ space: a.spaceId });

/** `agent_members`, with each agent's DM summary when `overview` (CUA-2266). */
export const agentMembersArgs = (a: ThreadMembersArgs): Json => (a.overview ? { space: a.spaceId, overview: true } : { space: a.spaceId });

/** `agent_set_rights` takes the member's name (names are unique per machine), not the Space. */
export function threadSetRightsArgs(a: ThreadSetRightsArgs): Json {
  const out: Json = { name: a.name };
  for (const k of ["task", "control", "approve"] as const) if (a[k] !== undefined) out[k] = a[k];
  return out;
}

/** `agent_remove` takes the member's name (names are unique per machine). */
export const threadAgentRemoveArgs = (a: ThreadAgentRemoveArgs): Json => ({ name: a.name });

/** `agent_add`: a started member of the Space (`agent` is the harness id). */
export const threadAgentAddArgs = (a: ThreadAgentAddArgs): Json => ({ space: a.spaceId, name: a.name, agent: a.harness });

export function threadJoinCodeCreateArgs(a: ThreadJoinCodeCreateArgs): Json {
  const out: Json = { space: a.spaceId };
  if (a.ttlS !== undefined) out.ttl_s = a.ttlS;
  if (a.canTask !== undefined) out.can_task = a.canTask;
  if (a.askBeforeJoin !== undefined) out.ask_before_join = a.askBeforeJoin;
  return out;
}

export const threadJoinCodeListArgs = (a: { spaceId: string }): Json => ({ space: a.spaceId });
export const threadJoinCodeCancelArgs = (a: { spaceId: string; id: string }): Json => ({ space: a.spaceId, id: a.id });

export function threadSettingsArgs(a: { spaceId: string; askBeforeJoin?: boolean }): Json {
  const out: Json = { space: a.spaceId };
  if (a.askBeforeJoin !== undefined) out.ask_before_join = a.askBeforeJoin;
  return out;
}

/** Each operation's daemon tool and arguments: what a native host runs. */
export const THREAD_TOOLS = {
  "thread.read": { tool: "thread_read", args: threadReadArgs },
  "thread.post": { tool: "thread_post", args: threadPostArgs },
  "thread.members": { tool: "agent_members", args: agentMembersArgs },
  "thread.setRights": { tool: "agent_set_rights", args: threadSetRightsArgs },
  "thread.agentAdd": { tool: "agent_add", args: threadAgentAddArgs },
  "thread.agentRemove": { tool: "agent_remove", args: threadAgentRemoveArgs },
  "thread.joinCodeCreate": { tool: "thread_join_code_create", args: threadJoinCodeCreateArgs },
  "thread.joinCodeList": { tool: "thread_join_code_list", args: threadJoinCodeListArgs },
  "thread.joinCodeCancel": { tool: "thread_join_code_cancel", args: threadJoinCodeCancelArgs },
  "thread.settings": { tool: "thread_settings", args: threadSettingsArgs },
} as const satisfies { [K in ThreadOpName]: { tool: string; args: (a: ThreadOps[K]["args"]) => Json } };

/* ---- Tool JSON (snake_case) -> the page's records ------------------------------------ */

const obj = (v: unknown): Json | null => (v && typeof v === "object" && !Array.isArray(v) ? (v as Json) : null);
const str = (v: unknown, d = "") => (typeof v === "string" ? v : d);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const strs = (v: unknown) => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);

const AUTHOR_KINDS: readonly ThreadAuthorKind[] = ["person", "agent", "system"];
const ENTRY_KINDS: readonly ThreadEntryKind[] = ["message", "task_update", "approval", "answer", "system", "join_request"];
const TASK_STATES: readonly TaskState[] = ["queued", "claimed", "working", "done", "failed"];

function author(v: unknown): ThreadAuthor {
  const a = obj(v) ?? {};
  const id = str(a.id);
  const kind = AUTHOR_KINDS.includes(a.kind as ThreadAuthorKind)
    ? (a.kind as ThreadAuthorKind)
    : id.startsWith("agent:")
      ? "agent"
      : id.startsWith("system:")
        ? "system"
        : "person";
  const name = str(a.name) || (kind === "agent" ? id.slice("agent:".length) : id);
  const identity = kind === "agent" ? memberIdentity(a.identity) : null;
  return identity ? { kind, id, name, identity } : { kind, id, name };
}

function ref(v: unknown): MemberRef | null {
  const r = obj(v);
  const id = str(r?.id);
  const name = str(r?.name);
  return r && (id || name) ? { id: id || name, name: name || id } : null;
}

const JOIN_METHODS: readonly JoinMethod[] = ["started", "prompt", "setup"];

/** A member's `identity` (CUA-2262), or null when the daemon sends none or no owner (the badge is never guessed). */
export function memberIdentity(v: unknown): MemberIdentity | null {
  const i = obj(v);
  const owner = ref(i?.owner);
  const handle = str(i?.handle);
  if (!i || !owner || !handle) return null;
  const h = obj(i.harness) ?? {};
  const joined = obj(i.joined) ?? {};
  const icon = str(i.icon);
  return {
    origin: i.origin === "external" ? "external" : "space",
    handle,
    displayName: str(i.display_name) || handle,
    collides: i.collides === true,
    // Only the harness's own mark or an https image: never a page-relative or data URL.
    icon: icon === "harness" || /^https:\/\//i.test(icon) ? icon : null,
    // `reported: false` from a started agent: Cua started it, so the harness is known; from a connected one: it didn't say.
    harness: { id: str(h.id), version: str(h.version) || null, reported: h.reported === true },
    owner,
    device: ref(i.device),
    joined: {
      method: JOIN_METHODS.includes(joined.method as JoinMethod) ? (joined.method as JoinMethod) : i.origin === "external" ? "prompt" : "started",
      atMs: num(joined.at_ms),
      by: ref(joined.by),
    },
    state: i.state === "pending" ? "pending" : "active",
    lastSeenMs: num(i.last_seen_ms),
  };
}

const EVENT_KINDS: readonly MemberEvent["kind"][] = ["member_joined", "member_removed", "name_refused", "rate_limited"];

function memberEvent(v: unknown): MemberEvent | null {
  const e = obj(v);
  if (!e || !EVENT_KINDS.includes(e.kind as MemberEvent["kind"])) return null;
  return { kind: e.kind as MemberEvent["kind"], member: memberIdentity(e.member), by: ref(e.by) };
}

function attachment(v: unknown): ThreadAttachment | null {
  const a = obj(v);
  if (!a) return null;
  const path = str(a.path);
  if (!path) return null;
  return { path, name: str(a.name) || path.slice(path.lastIndexOf("/") + 1), size: num(a.size), mime: str(a.mime) || null };
}

/** One entry of `thread_read`/`thread_post`, as the page reads it. Unknown
 * kinds read as plain messages (a newer daemon's additions still show). */
export function threadEntryFromTool(v: unknown): ThreadEntry | null {
  const e = obj(v);
  const seq = num(e?.seq);
  if (!e || seq === null) return null;
  const task = obj(e.task);
  const approval = obj(e.approval);
  const answer = obj(e.answer);
  const update = obj(e.update);
  const kind = ENTRY_KINDS.includes(e.kind as ThreadEntryKind) ? (e.kind as ThreadEntryKind) : "message";
  const updateState = TASK_STATES.includes(update?.state as TaskState) ? (update!.state as TaskState) : null;
  return {
    seq,
    id: str(e.id) || String(seq),
    tsMs: num(e.ts_ms) ?? 0,
    author: author(e.author),
    kind,
    text: str(e.text),
    mentions: strs(e.mentions),
    replyTo: num(e.reply_to),
    task: task && str(task.to) ? { to: str(task.to), session: num(task.session) } : null,
    noteFor: kind === "system" ? num(e.task_seq) : null,
    attachments: (Array.isArray(e.attachments) ? e.attachments : []).map(attachment).filter((a): a is ThreadAttachment => a !== null),
    approval: approval ? { action: str(approval.action), details: str(approval.details), state: str(approval.state, "open") } : null,
    answer: answer && num(answer.approval) !== null ? { approval: num(answer.approval)!, approve: answer.approve === true, note: str(answer.note) || null } : null,
    update:
      update && num(update.task) !== null && updateState
        ? { task: num(update.task)!, state: updateState, session: num(update.session), note: str(update.note) || null }
        : null,
    runsOnKeysOf: keyOwner(e.runs_on_keys_of),
    event: kind === "system" ? memberEvent(e.event) : null,
    ...(kind === "join_request" ? { joinRequest: joinRequest(e) } : {}),
    ...(e.view != null ? { view: threadViewFromTool(e.view) } : {}),
    ...(e.failure != null ? { failure: threadFailure(e.failure) } : {}),
  };
}

/** A typed failure (`failure` of an entry, or of a failed member's status). */
export function threadFailure(v: unknown): ThreadFailure | null {
  const f = obj(v);
  if (!f) return null;
  return {
    class: f.class === "needs_connect" ? "needs_connect" : "error",
    provider: str(f.provider) || null,
    message: str(f.message),
    retryable: f.retryable !== false,
  };
}

function joinRequest(e: Record<string, unknown>): JoinRequest | null {
  const j = obj(e.join_request);
  if (!j) return null;
  return {
    member: memberIdentity(j.member),
    canTask: j.can_task === "members" ? "members" : "owner",
    state: str(j.state, "open"),
    codeId: str(j.code_id),
    youCanAnswer: e.you_can_answer === true,
  };
}

const CODE_STATES: readonly JoinCodeState[] = ["open", "pending_approval", "redeemed", "denied", "expired", "cancelled"];

/** `thread_join_code_create`'s JSON (empty strings for what a daemon left out, as the other mappers do). */
export function joinCodeFromTool(v: unknown): JoinCode {
  const c = obj(v) ?? {};
  return { id: str(c.id), code: str(c.code), url: str(c.url), line: str(c.line), expiresAtMs: num(c.expires_at_ms) ?? 0 };
}

/** `thread_join_code_list`'s JSON (`{codes}` or a bare list). */
export function joinCodesFromTool(v: unknown): { codes: JoinCodeRow[] } {
  const raw = Array.isArray(v) ? v : obj(v)?.codes;
  const codes = (Array.isArray(raw) ? raw : []).flatMap((r): JoinCodeRow[] => {
    const c = obj(r);
    const id = str(c?.id);
    if (!c || !id) return [];
    return [
      {
        id,
        createdAtMs: num(c.created_at_ms) ?? 0,
        expiresAtMs: num(c.expires_at_ms) ?? 0,
        state: CODE_STATES.includes(c.state as JoinCodeState) ? (c.state as JoinCodeState) : "expired",
        canTask: c.can_task === "members" ? "members" : "owner",
        askBeforeJoin: c.ask_before_join === true,
        createdBy: ref(c.created_by),
        memberHandle: str(c.member_handle) || null,
      },
    ];
  });
  return { codes };
}

/** `thread_settings`' JSON. */
export function threadSettingsFromTool(v: unknown): ThreadSettings {
  return { askBeforeJoin: obj(v)?.ask_before_join !== false };
}

/** `agent_add`'s JSON (the row, or `{member}`). */
export function addedMemberFromTool(v: unknown): AgentMember | null {
  const o = obj(v);
  return agentMemberFromTool(o && obj(o.member) ? o.member : v);
}

function keyOwner(v: unknown): { id: string; name: string } | null {
  const k = obj(v);
  const id = str(k?.id);
  return k && id ? { id, name: str(k.name) || id } : null;
}

const ROLES: readonly ThreadRole[] = ["owner", "admin", "member", "guest", "agent"];

function holders(v: unknown): RightHolders | null {
  if (v === "owner" || v === "admins" || v === "members") return v;
  return Array.isArray(v) ? strs(v) : null;
}

function rights(v: unknown): AgentRights | null {
  const r = obj(v);
  if (!r) return null;
  return {
    task: holders(r.task) ?? DEFAULT_AGENT_RIGHTS.task,
    control: holders(r.control) ?? DEFAULT_AGENT_RIGHTS.control,
    approve: holders(r.approve) ?? DEFAULT_AGENT_RIGHTS.approve,
  };
}

function youCan(v: unknown): Record<AgentRight, boolean> | null {
  const y = obj(v);
  return y ? { task: y.task === true, control: y.control === true, approve: y.approve === true } : null;
}

/** `dm` and `needs_you` on an `agent_members {overview}` row (absent otherwise). */
function agentOverview(m: Json): { dm?: AgentDm | null; needsYou?: NeedsYou[] } {
  if (!("dm" in m) && !("needs_you" in m)) return {};
  const d = obj(m.dm);
  const last = obj(d?.last);
  const needs = Array.isArray(m.needs_you) ? m.needs_you : [];
  return {
    dm: d ? { unread: num(d.unread) ?? 0, last: last && num(last.seq) !== null ? { seq: num(last.seq)!, tsMs: num(last.ts_ms) ?? 0, text: str(last.text), author: author(last.author) } : null } : null,
    needsYou: needs.flatMap((n) => {
      const x = obj(n);
      const seq = num(x?.seq);
      if (!x || seq === null || (x.kind !== "approval" && x.kind !== "question")) return [];
      return [{ seq, kind: x.kind, text: str(x.text), tsMs: num(x.ts_ms) ?? 0 }];
    }),
  };
}

/** `thread_read`'s answer. */
export function threadPageFromTool(v: unknown): ThreadPage {
  const p = obj(v) ?? {};
  const entries = (Array.isArray(p.entries) ? p.entries : []).map(threadEntryFromTool).filter((e): e is ThreadEntry => e !== null);
  const last = entries.length ? entries[entries.length - 1]!.seq : 0;
  const me = obj(p.me);
  return {
    entries,
    cursor: num(p.cursor) ?? last,
    caughtUp: p.caught_up !== false,
    unread: num(p.unread) ?? 0,
    readSeq: num(p.read_seq),
    me: (me ? str(me.id) : str(p.me)) || null,
    hasMoreBefore: p.has_more_before === true,
    role: ROLES.includes(p.role as ThreadRole) ? (p.role as ThreadRole) : null,
  };
}

/** `thread_post`'s answer: the entry it wrote (bare, or under `entry`). */
export function threadPostFromTool(v: unknown): ThreadEntry | null {
  const p = obj(v);
  return threadEntryFromTool(p && "entry" in p ? p.entry : v);
}

/** `agent_members`' answer (`{members}` or a bare list). */
export function agentMembersFromTool(v: unknown): { members: AgentMember[] } {
  const rows = Array.isArray(v) ? v : Array.isArray(obj(v)?.members) ? (obj(v)!.members as unknown[]) : [];
  return { members: rows.flatMap((r) => agentMember(r)) };
}

/** `agent_set_rights`' answer: the member (with its new rights). */
export function agentMemberFromTool(v: unknown): AgentMember | null {
  return agentMember(v)[0] ?? null;
}

function agentMember(r: unknown): AgentMember[] {
  const m = obj(r);
  const name = str(m?.name);
  if (!m || !name) return [];
  const status = obj(m.status);
  return [
    {
      name,
      ...(str(m.handle) && str(m.handle) !== name ? { handle: str(m.handle) } : {}),
      harness: str(m.harness),
      // `status` is a word, or `{state, activity}`.
      status: (status ? str(status.state) : str(m.status)) || "idle",
      activity: (status ? str(status.activity) : str(m.activity)) || null,
      ...(status && status.failure != null ? { failure: threadFailure(status.failure) } : {}),
      sessions: num(m.sessions) ?? 1,
      rights: rights(m.rights),
      youCan: youCan(m.you_can),
      heldBy: str(obj(m.held_by)?.name) || null,
      identity: memberIdentity(m.identity),
      ...agentOverview(m),
    },
  ];
}

/* ---- An old daemon ---------------------------------------------------------------------- */

/** The daemon (or this app's SDK) has no thread tools: the SDK's
 * `NotFound("Spaces tool thread_read")`, or the MCP server's
 * `unknown tool thread_read`. A Space that is not there is a different
 * error and still shows. */
export function isMissingThreadTool(e: unknown): boolean {
  if (isUnsupported(e)) return true;
  const message = e instanceof Error ? e.message : typeof e === "string" ? e : "";
  return /\b(unknown tool|no such tool)\b.*\b(thread_\w+|agent_members)\b|\bspaces tool (thread_\w+|agent_members)\b/i.test(message);
}

/** The thread's tool failed because the daemon has none: `unsupported`, the
 * page's signal to keep the old layout. Native hosts use the same test. */
export function threadToolError(e: unknown): unknown {
  if (!isMissingThreadTool(e) || isUnsupported(e)) return e;
  return new HostError(e instanceof Error ? e.message : String(e), "unsupported");
}

/* ---- Hosts ---------------------------------------------------------------------------- */

type Handlers = { [K in ThreadOpName]: (args: ThreadOps[K]["args"]) => Promise<ThreadOps[K]["result"]> };
type Request = <T>(method: ThreadWebkitMethod, args?: Record<string, unknown>) => Promise<T>;

/** The SwiftUI host and the Electron shell: the operation's own name and
 * arguments; the answer is the tool's JSON, mapped here. */
export function webkitThreadOps(request: Request): Handlers {
  const call = async (op: ThreadWebkitMethod, args: object) => {
    try {
      return await request<unknown>(op, { ...args });
    } catch (e) {
      throw threadToolError(e);
    }
  };
  return {
    "thread.read": async (a) => threadPageFromTool(await call("thread.read", a)),
    "thread.post": async (a) => threadPostFromTool(await call("thread.post", a)),
    "thread.members": async (a) => agentMembersFromTool(await call("thread.members", a)),
    "thread.setRights": async (a) => agentMemberFromTool(await call("thread.setRights", a)),
    "thread.agentAdd": async (a) => addedMemberFromTool(await call("thread.agentAdd", a)),
    "thread.agentRemove": async (a) => {
      await call("thread.agentRemove", a);
      return null;
    },
    "thread.joinCodeCreate": async (a) => joinCodeFromTool(await call("thread.joinCodeCreate", a)),
    "thread.joinCodeList": async (a) => joinCodesFromTool(await call("thread.joinCodeList", a)),
    "thread.joinCodeCancel": async (a) => {
      await call("thread.joinCodeCancel", a);
      return null;
    },
    "thread.settings": async (a) => threadSettingsFromTool(await call("thread.settings", a)),
  };
}

/** A host with no daemon thread (Tauri). */
export function unsupportedThreadOps(mode: BridgeMode): Handlers {
  const no = (op: ThreadOpName) => (): Promise<never> => Promise.reject(new UnsupportedOperationError(mode, op));
  return Object.fromEntries(THREAD_OPERATIONS.map((op) => [op, no(op)])) as unknown as Handlers;
}
