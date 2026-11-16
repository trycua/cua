// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The sample-data backend: in-memory, deterministic, and labelled as such on
 * every screen (`capabilities.sample`). It exists so the app can be seen and
 * tested without an account; it is not offered in a release build unless the
 * developer switch is on. Its "agent" answers are canned text, made up.
 */

import type { TaskRequest } from "../../core/phone";
import { summarize, type SpaceInput } from "../summary";
import {
  changesFromTool,
  fileDiffFromTool,
  filePreviewFromTool,
  mergeTaskOptions,
  mergeToolRows,
  offlineMachineOptions,
  taskOptionsFromTool,
  taskResultFromTool,
  toolRowsFromTool,
  type ToolGroup,
} from "../thread/phone-protocol";
import type { AgentMember, Backend, InputAction, SpaceList, StreamTicket, TaskResult, ThreadEntry, ThreadPage } from "../types";
import * as D from "./data";

interface State {
  entries: Map<string, ThreadEntry[]>;
  members: Map<string, AgentMember[]>;
  read: Map<string, number>;
  asleep: Set<string>;
  controlled: Set<string>;
  live: Map<string, { agent: string; stage: string; text?: string }[]>;
  /** Tool rows by Space, in the daemon's wire shape (read through `toolRowsFromTool`, like a live answer). */
  tools: Map<string, ToolGroup[]>;
  /** Messages waiting behind the running turn, by Space. */
  queued: Map<string, number>;
  /** A steer bumps this; the turn it interrupts sees the change and stops. */
  epoch: Map<string, number>;
  /** Replies run one at a time per Space. */
  chain: Map<string, Promise<void>>;
  /** Tasks started, by client id: the same id twice is one task. */
  tasks: Map<string, TaskResult>;
  /** Client ids already posted: the same id twice is one entry. */
  posted: Map<string, ThreadEntry>;
  /** When a Space being made for a task is ready, by client id. */
  ready: Map<string, number>;
}

export interface DemoOptions {
  now?: () => number;
  /** Streaming reply pace, ms per word (tests set 0). */
  wordMs?: number;
  /** Start as if the phone had no connection (the sample build's switch in Settings). */
  offline?: boolean;
  /** How long making a new Space for a task takes, ms (tests set 0; the sample build shows the "starting" screen for a moment). */
  createMs?: number;
  /** How long a sample agent's last tool call shows as running, ms (default 2500 in the sample build, 0 when `wordMs` is 0). */
  toolMs?: number;
}

/** What the sample backend recorded, for tests and for the screenshots' honest labels. */
export interface DemoRecord {
  inputs: InputAction[];
  /** Push registrations this phone made (the mock push provider's inbox). */
  pushRegistrations: Record<string, unknown>[];
  pushRemoved: number;
  /** Tasks started, in order. */
  started: TaskRequest[];
}

export function createDemoBackend(opts: DemoOptions = {}): Backend & { inputs: InputAction[]; record: DemoRecord } {
  let offline = false;
  /** While the sample phone pretends to be offline every call fails the way a lost connection does. */
  const down = () => {
    if (offline) throw new TypeError("Network request failed");
  };
  offline = opts.offline ?? false;
  const now = opts.now ?? Date.now;
  const wordMs = opts.wordMs ?? 70;
  const toolMs = opts.toolMs ?? (wordMs === 0 ? 0 : 2_500);
  const t0 = now();
  const machines = D.machines(t0);
  const sp = D.spaces(t0);
  const st: State = {
    entries: new Map(),
    members: new Map(),
    read: new Map(),
    asleep: new Set(),
    controlled: new Set(),
    live: new Map(),
    tools: new Map(),
    queued: new Map(),
    epoch: new Map(),
    chain: new Map(),
    tasks: new Map(),
    posted: new Map(),
    ready: new Map(),
  };
  const record: DemoRecord = { inputs: [], pushRegistrations: [], pushRemoved: 0, started: [] };
  const own = (id: string, e: ThreadEntry[], unread: number) => {
    st.entries.set(id, e);
    st.read.set(id, Math.max(0, (e[e.length - 1]?.seq ?? 0) - unread));
  };
  own("relay:sample-checkout-api", D.checkoutThread(t0), 2);
  own("relay:sample-design-review", D.designThread(t0), 0);
  own(
    "relay:sample-ledger",
    D.simpleThread(t0, D.BOOKKEEPER, ["Matched 41 of 44 March invoices.", "3 invoices have no purchase order. I listed them in the sheet."]),
    1,
  );
  own("relay:sample-desktop-pc", D.simpleThread(t0, D.OPERATOR, ["Nothing to do right now. Message me when you need the PC."]), 0);
  own("relay:sample-web-research", D.simpleThread(t0, D.RESEARCHER, ["Three reports read. Drafting the summary now."]), 0);
  own("relay:sample-ci-runner", D.simpleThread(t0, D.CLAUDE, ["Runner idle since yesterday."]), 0);
  for (const s of sp) st.members.set(s.id, s.members);
  st.tools.set("relay:sample-checkout-api", toolRowsFromTool(D.CHECKOUT_TOOL_ROWS_WIRE).groups);
  const inputs: SpaceInput[] = [];
  const inputs_action: InputAction[] = record.inputs;

  const build = (): SpaceInput[] =>
    sp.map((s) => {
      const e = st.entries.get(s.id) ?? [];
      const last = [...e].reverse().find((x) => x.kind === "message" || x.kind === "approval") ?? null;
      const readSeq = st.read.get(s.id) ?? 0;
      const members = (st.members.get(s.id) ?? []).map((m) => (st.asleep.has(s.id) ? { ...m, status: "asleep", activity: null } : m));
      return {
        id: s.id,
        name: s.name,
        machine: machines.find((m) => m.id === s.machineId)!,
        scope: s.scope,
        kindLabel: s.kindLabel,
        scene: s.scene,
        role: s.role,
        members,
        people: s.people,
        last,
        unread: e.filter((x) => x.seq > readSeq && x.author.id !== D.ME.id).length,
        canSleep: s.members.length > 0,
        canShare: s.role === "owner",
      };
    });

  const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
  const nextSeq = (id: string) => (st.entries.get(id)?.at(-1)?.seq ?? 0) + 1;

  /** The reply to a message to an agent: it wakes, reads and runs (tool rows appear as it goes), then streams its words. */
  async function agentReplies(spaceId: string, to: string, ask: ThreadEntry) {
    const m = st.members.get(spaceId)?.find((x) => x.name === to);
    if (!m) return;
    const epoch = st.epoch.get(spaceId) ?? 0;
    const stale = () => (st.epoch.get(spaceId) ?? 0) !== epoch;
    const wake = st.asleep.has(spaceId) || m.status === "asleep";
    if (wake) {
      st.asleep.delete(spaceId);
      st.members.set(
        spaceId,
        (st.members.get(spaceId) ?? []).map((x) => (x.name === to ? { ...x, status: "working", activity: "reading your message" } : x)),
      );
      // No thread note: a start the person asked for shows as the starting row.
    }
    const text = `Got it: "${ask.text
      .split("\n")[0]!
      .replace(/@\S+\s*/g, "")
      .slice(0, 60)}". I'm on it and will post here when it is done.`;
    st.live.set(spaceId, [{ agent: to, stage: "thinking" }]);
    await sleep(wordMs * 6);
    if (stale()) return;
    // Its tool calls land as rows after the message it answers: the last one runs for a moment, then is done.
    const rows = toolRowsFromTool({ groups: [{ after_seq: ask.seq, agent: to, calls: D.replyToolCalls(ask.seq) }], cursor: ask.seq, caught_up: true }).groups;
    st.tools.set(spaceId, mergeToolRows(st.tools.get(spaceId) ?? [], rows));
    st.live.set(spaceId, [{ agent: to, stage: "using Bash" }]);
    await sleep(Math.max(wordMs * 6, toolMs));
    if (stale()) return;
    const done = D.replyToolCalls(ask.seq).map((c) => ({ ...c, state: "done" }));
    st.tools.set(spaceId, mergeToolRows(st.tools.get(spaceId) ?? [], toolRowsFromTool({ groups: [{ after_seq: ask.seq, agent: to, calls: done }] }).groups));
    const words = text.split(" ");
    let shown = "";
    for (const w of words) {
      if (stale()) return;
      shown = shown ? `${shown} ${w}` : w;
      st.live.set(spaceId, [{ agent: to, stage: "replying", text: shown }]);
      await sleep(wordMs);
    }
    st.live.set(spaceId, []);
    st.entries
      .get(spaceId)!
      .push(D.entry(nextSeq(spaceId), now(), { kind: "agent", id: `agent:${to}`, name: m.identity?.displayName ?? to }, text, { replyTo: ask.seq }));
  }

  /** Runs replies one at a time per Space: a queued message waits for the turn in progress. */
  function enqueueReply(spaceId: string, to: string, ask: ThreadEntry) {
    st.queued.set(spaceId, (st.queued.get(spaceId) ?? 0) + (st.chain.has(spaceId) ? 1 : 0));
    const was = st.chain.get(spaceId) ?? Promise.resolve();
    const next: Promise<void> = was
      .then(() => {
        st.queued.set(spaceId, Math.max(0, (st.queued.get(spaceId) ?? 0) - 1));
        return agentReplies(spaceId, to, ask);
      })
      .catch(() => {})
      .finally(() => {
        if (st.chain.get(spaceId) === next) st.chain.delete(spaceId);
      });
    st.chain.set(spaceId, next);
  }

  const withQueued = (spaceId: string, ms: AgentMember[]): AgentMember[] => {
    const q = st.queued.get(spaceId) ?? 0;
    return q > 0 ? ms.map((m, i) => (i === 0 ? { ...m, queued: q } : m)) : ms;
  };

  let taskN = 0;
  const createMs = opts.createMs ?? 0;
  const pendingReq = new Map<string, TaskRequest>();

  /** Makes the Space for a task (or finds the one named) and posts the job as the first message, like the daemon's `task_start`. */
  function finishTask(req: TaskRequest): TaskResult {
    const target = req.where.kind === "space" ? req.where.space : null;
    let spaceId = target;
    if (!spaceId) {
      taskN += 1;
      spaceId = `relay:sample-task-${taskN}`;
      const machineId = req.where.kind === "machine" ? req.where.machine : "studio-mac";
      const handle = req.agent === "openai-codex" ? "codex" : req.agent === "hermes" ? "hermes" : req.agent === "openclaw" ? "openclaw" : "claude";
      const label = handle === "claude" ? "Claude Code" : handle === "codex" ? "Codex" : handle === "hermes" ? "Hermes" : "OpenClaw";
      const name =
        req.prompt
          .split(/\s+/)
          .slice(0, 3)
          .join("-")
          .toLowerCase()
          .replace(/[^a-z0-9-]/g, "") || `task-${taskN}`;
      const m: AgentMember = {
        name: handle,
        handle,
        harness: req.agent,
        status: "asleep",
        activity: null,
        sessions: 1,
        rights: null,
        youCan: null,
        heldBy: null,
        identity: {
          origin: "space",
          handle,
          displayName: label,
          collides: false,
          icon: null,
          harness: { id: req.agent, version: null, reported: true },
          owner: { id: D.ME.id, name: D.ME.name },
          device: null,
          joined: { method: "started", atMs: null, by: null },
          state: "active",
          lastSeenMs: null,
        },
        dm: { unread: 0, last: null },
        needsYou: [],
      };
      sp.push({
        id: spaceId,
        name,
        machineId,
        scope: "machine",
        kindLabel: "Linux",
        scene: "terminal",
        role: "owner",
        members: [m],
        people: [{ id: D.ME.id, name: D.ME.name, kind: "person", harness: null, status: "here", activity: null, role: "Owner", here: true }],
      });
      st.members.set(spaceId, [m]);
      st.entries.set(spaceId, []);
      st.read.set(spaceId, 0);
    }
    // The machine adds the `Repo:` line to the job it posts.
    const text = req.repo ? `${req.message}\n\nRepo: ${req.repo.url}` : req.message;
    const e = D.entry(nextSeq(spaceId), now(), D.ME, text, { clientId: req.client_id });
    const mention = /@([a-z0-9._-]+)/i.exec(req.message);
    if (mention) {
      e.mentions = [mention[1]!];
      e.task = { to: mention[1]!, session: null };
    }
    st.entries.get(spaceId)!.push(e);
    if (mention) enqueueReply(spaceId, mention[1]!, e);
    const r = taskResultFromTool({ space: spaceId, created: !target, entry_seq: e.seq, state: "ready", agent: mention?.[1] ?? null });
    st.tasks.set(req.client_id, r);
    pendingReq.delete(req.client_id);
    return r;
  }
  /** A Space that was being made is ready. */
  function settle(clientId: string): TaskResult {
    const req = pendingReq.get(clientId);
    return req ? finishTask(req) : st.tasks.get(clientId)!;
  }

  return {
    kind: "demo",
    inputs: inputs_action,
    record,
    sample: {
      get offline() {
        return offline;
      },
      setOffline(on: boolean) {
        offline = on;
      },
    },
    capabilities: {
      sample: true,
      stream: "viewer",
      input: true,
      sleep: true,
      share: true,
      startTask: true,
      diff: true,
      scopes: true,
      steer: true,
      push: true,
      why: {},
    },
    async listSpaces(): Promise<SpaceList> {
      down();
      void inputs;
      return summarize(build(), machines);
    },
    async readThread(spaceId, o): Promise<ThreadPage> {
      down();
      const pick = () => {
        const all = st.entries.get(spaceId) ?? [];
        if (o.after != null) return all.filter((e) => e.seq > o.after!);
        if (o.before != null) return all.filter((e) => e.seq < o.before!);
        return all;
      };
      // A long poll: wait a little, then read again (the real daemon waits up to `followMs` for news).
      if (o.followMs && o.after != null && !pick().length) {
        const until = Date.now() + Math.min(o.followMs, 400);
        while (!pick().length && Date.now() < until) await sleep(25);
      }
      const all = st.entries.get(spaceId) ?? [];
      const entries = pick();
      const readSeq = st.read.get(spaceId) ?? 0;
      if (o.markRead) st.read.set(spaceId, all.at(-1)?.seq ?? 0);
      const live = st.live.get(spaceId) ?? [];
      return {
        entries,
        live: { key: JSON.stringify(live), agents: live },
        cursor: all.at(-1)?.seq ?? 0,
        caughtUp: true,
        unread: all.filter((e) => e.seq > readSeq && e.author.id !== D.ME.id).length,
        readSeq,
        me: D.ME.id,
        hasMoreBefore: false,
        role: "owner",
      };
    },
    async members(spaceId) {
      down();
      return withQueued(
        spaceId,
        (st.members.get(spaceId) ?? []).map((m) => (st.asleep.has(spaceId) ? { ...m, status: "asleep", activity: null } : m)),
      );
    },
    async post(spaceId, text, o) {
      down();
      if (o?.clientId && st.posted.has(o.clientId)) return st.posted.get(o.clientId)!;
      const e = D.entry(nextSeq(spaceId), now(), D.ME, text.trim(), { replyTo: o?.replyTo ?? null });
      if (o?.clientId) {
        e.clientId = o.clientId;
        st.posted.set(o.clientId, e);
      }
      const mention = /@([a-z0-9._-]+)/i.exec(text);
      if (mention) {
        e.mentions = [mention[1]!];
        e.task = { to: mention[1]!, session: null };
      }
      // Steer: the turn in progress is cut short and this message goes now.
      if (o?.mode === "steer") {
        st.epoch.set(spaceId, (st.epoch.get(spaceId) ?? 0) + 1);
        st.chain.delete(spaceId);
        st.queued.set(spaceId, 0);
        st.live.set(spaceId, []);
      }
      st.entries.get(spaceId)!.push(e);
      if (mention && !st.controlled.has(spaceId)) enqueueReply(spaceId, mention[1]!, e);
      return e;
    },
    async answer(spaceId, approval, approve, o) {
      down();
      if (o?.clientId && st.posted.has(o.clientId)) return st.posted.get(o.clientId)!;
      const e = D.entry(nextSeq(spaceId), now(), D.ME, approve ? (o?.scope === "space" ? "Allowed for this Space" : "Approved") : "Denied", {
        kind: "answer",
        answer: { approval, approve, note: null, scope: approve ? (o?.scope ?? "once") : null },
      });
      if (o?.clientId) {
        e.clientId = o.clientId;
        st.posted.set(o.clientId, e);
      }
      st.entries.get(spaceId)!.push(e);
      const space = st.members.get(spaceId) ?? [];
      // The agent's `needsYou` loses the one answered; it goes back to work once nothing else waits.
      st.members.set(
        spaceId,
        space.map((m) => {
          const left = (m.needsYou ?? []).filter((n) => n.seq !== approval);
          return left.length
            ? { ...m, needsYou: left }
            : { ...m, status: approve ? "working" : "idle", activity: approve ? "pushing the branch" : null, needsYou: [] };
        }),
      );
      return e;
    },
    async taskOptions() {
      down();
      const names = new Map(machines.map((m) => [m.id, m.name]));
      const merged = mergeTaskOptions(Object.values(D.SAMPLE_TASK_OPTIONS_BY_HOST).map(taskOptionsFromTool), names);
      return { ...merged, machines: [...merged.machines, ...offlineMachineOptions(machines, new Set(merged.machines.map((m) => m.id)))] };
    },
    async taskStatus(clientId) {
      down();
      const r = st.tasks.get(clientId);
      if (!r) throw new Error("no task was started with this client_id");
      return r.state === "creating" && Date.now() >= (st.ready.get(clientId) ?? 0) ? settle(clientId) : r;
    },
    async startTask(req) {
      down();
      const had = st.tasks.get(req.client_id);
      if (had) return { ...had, created: false };
      record.started.push(req);
      if (req.where.kind === "space") return finishTask(req);
      if (createMs <= 0) return finishTask(req);
      // A new Space takes a moment (an image to pull): the call answers `creating` at once and `taskStatus` follows it.
      const r = taskResultFromTool({ space: "", created: true, state: "creating", pending_id: req.client_id });
      st.tasks.set(req.client_id, r);
      pendingReq.set(req.client_id, req);
      st.ready.set(req.client_id, Date.now() + createMs);
      setTimeout(() => void settle(req.client_id), createMs + 5);
      return r;
    },
    async changes(spaceId) {
      down();
      if (spaceId !== "relay:sample-checkout-api") return changesFromTool({ repo: "", branch: "", files: [] });
      return changesFromTool(D.SAMPLE_CHANGES_WIRE);
    },
    async fileDiff(_spaceId, path) {
      down();
      const f = D.SAMPLE_CHANGES_WIRE.files.find((x) => x.path === path);
      return fileDiffFromTool({ path, patch: D.SAMPLE_PATCHES[path] ?? "", binary: f?.binary ?? false, truncated: false });
    },
    async filePreview(_spaceId, path) {
      down();
      const p = D.SAMPLE_PREVIEWS[path];
      if (!p) throw new Error("This file has no preview in the sample data.");
      return filePreviewFromTool({ path, mime: p.mime, size: p.size, encoding: p.encoding, content: p.content });
    },
    push: {
      // The sample phone has no relay behind it: it keeps what it would register, and Settings says push to a closed app is not set up.
      async status() {
        down();
        return { configured: false };
      },
      async register(body) {
        record.pushRegistrations.push(body);
      },
      async unregister() {
        record.pushRemoved += 1;
      },
    },
    async streamTicket(): Promise<StreamTicket | null> {
      return { viewerUrl: "about:blank#sample", expiresAtMs: Date.now() + 3_600_000, width: 1440, height: 900 };
    },
    async sendInput(_spaceId, action) {
      inputs_action.push(action);
    },
    async takeControl(spaceId, on) {
      if (on) st.controlled.add(spaceId);
      else st.controlled.delete(spaceId);
      st.members.set(
        spaceId,
        (st.members.get(spaceId) ?? []).map((m) =>
          on && m.status === "working"
            ? { ...m, status: "paused", activity: "paused while you drive" }
            : !on && m.status === "paused"
              ? { ...m, status: "working", activity: "back at it" }
              : m,
        ),
      );
    },
    async stopAgent(spaceId) {
      st.live.set(spaceId, []);
      st.members.set(
        spaceId,
        (st.members.get(spaceId) ?? []).map((m) => (m.status === "working" ? { ...m, status: "idle", activity: null } : m)),
      );
    },
    async toolRows(spaceId) {
      down();
      // The daemon's wire shape, read through the same mapper as a live answer.
      const groups = st.tools.get(spaceId) ?? [];
      return toolRowsFromTool({
        groups: groups.map((g) => ({
          after_seq: g.afterSeq,
          agent: g.agent,
          calls: g.calls.map((c) => ({ id: c.id, cat: c.cat, line: c.line, detail: c.detail, state: c.state, at_ms: c.atMs })),
        })),
        // A counter that moves whenever a call is added or finishes, like the daemon's.
        cursor: groups.flatMap((g) => g.calls).reduce((n, c) => n + 10 + (c.state === "running" ? 0 : 1), 0),
        caught_up: true,
      });
    },
    async sleep(spaceId) {
      st.asleep.add(spaceId);
    },
    async share() {},
  };
}
