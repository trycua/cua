// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The live backend: the signed-in account's machines from the relay, and each
 * Space's thread, stream and input through the machine that provides it.
 *
 * Verified in `__tests__/live-backend.test.ts` against a fake relay and a
 * fake daemon; NOT verified against a live relay (no account was available
 * to the build). What the relay or the daemon does not offer to a phone is
 * a missing capability with its reason, never a stand-in.
 */

import { create } from "@bufbuild/protobuf";

import type { TaskRequest } from "../../core/phone";
import { summarize, type SpaceInput } from "../summary";
import {
  changesArgs,
  changesFromTool,
  fileDiffArgs,
  fileDiffFromTool,
  filePreviewArgs,
  filePreviewFromTool,
  mergeTaskOptions,
  offlineMachineOptions,
  taskOptionsFromTool,
  taskResultFromTool,
  taskStatusArgs,
  toolRowsArgs,
  toolRowsFromTool,
} from "../thread/phone-protocol";
import {
  agentMembersFromTool,
  threadMembersArgs,
  threadPageFromTool,
  threadPostArgs,
  threadPostFromTool,
  threadReadArgs,
  agentMembersArgs,
  isMissingThreadTool,
  type AgentMember,
  type ThreadEntry,
  type ThreadPage,
} from "../thread/protocol";
import { errorText } from "../../store/network";
import { ControlNotAllowedError, NeedsUpdateError, OLD_COMPUTER_REFUSAL } from "../types";
import type { Backend, Capabilities, InputAction, MachineInfo, SpaceList, StreamTicket, TaskOptions } from "../types";
import { CoordinateSpace } from "./gen/cua/env/v1/common_pb";
import { SessionPolicy } from "./gen/cua/env/v1/stream_pb";
import { Key, KeyInputSchema, MouseButton, ScrollUnit } from "./gen/cua/env/v1/computer_pb";
import { clients, transportFor, type Credentials, type FetchLike } from "./grpc";
import type { RelayClient } from "./relay";
import { directory, placementOffered, type ComputerRow } from "../../core/views";
import { mapDirectory, type SpaceMachine } from "./spaces";

export interface LiveDeps {
  relay: RelayClient;
  /** The account's access token (refreshing) and this device's relay session. */
  token(): Promise<string>;
  deviceSession(): Promise<string | null>;
  /** The reader's account id (`sub`), for roles. */
  me(): string | null;
  /** This phone's relay device id (`dev_...`), for push registration. */
  deviceId?(): string | null;
  fetcher?: FetchLike;
  now?: () => number;
}

const KEYS: Record<string, Key> = {
  escape: Key.ESCAPE,
  tab: Key.TAB,
  return: Key.ENTER,
  up: Key.ARROW_UP,
  down: Key.ARROW_DOWN,
  left: Key.ARROW_LEFT,
  right: Key.ARROW_RIGHT,
  ctrl: Key.CONTROL,
  alt: Key.ALT,
  cmd: Key.META,
};

/** The text of a thread tool's MCP content parts (`content_json`), parsed as the tool's JSON. */
export function toolJson(contentJson: string, isError: boolean): unknown {
  const parts = JSON.parse(contentJson || "[]") as { type?: string; text?: string }[];
  const text = parts
    .filter((p) => p.type === "text")
    .map((p) => p.text ?? "")
    .join("\n");
  if (isError) throw new Error(text || "the daemon refused");
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

/** How long a live-desktop ticket lives; the page is loaded again with a new one shortly before. */
export const STREAM_TICKET_SECONDS = 300;

export function createLiveBackend(d: LiveDeps): Backend {
  const now = d.now ?? Date.now;
  const creds: Credentials = {
    async headers() {
      const h: Record<string, string> = { authorization: `Bearer ${await d.token()}` };
      const s = await d.deviceSession();
      if (s) h["x-cua-device-session"] = s;
      return h;
    },
  };
  let spaces = new Map<string, SpaceMachine>();
  let hosts = new Set<string>();
  let machinesKnown: MachineInfo[] = [];
  /** The pool as last read: computers and phones, with what each says it can host. */
  let computersKnown: ComputerRow[] = [];
  /** The machine each task was started on, to ask it how the task is coming (`task_status`). */
  const taskHosts = new Map<string, string>();
  const cache = new Map<string, ReturnType<typeof clients>>();
  const clientsFor = (machineId: string, extra: Record<string, string> = {}) => {
    const k = machineId + JSON.stringify(extra);
    let c = cache.get(k);
    if (!c) {
      c = clients(transportFor(d.relay.machineUrl(machineId), creds, extra, d.fetcher));
      cache.set(k, c);
    }
    return c;
  };
  const spaceOf = (id: string): SpaceMachine => {
    const s = spaces.get(id);
    if (!s) throw new Error("This Space is not in your list any more. Pull to refresh.");
    return s;
  };
  const idOf = (s: SpaceMachine) => `relay:${s.machine.id}`;

  /** One thread or agent member tool, run by the daemon of the machine that provides the Space (`HostThreadService`). */
  /** A refusal because the computer predates phones becomes the plain "needs an update"; anything else passes. */
  const needsUpdate =
    (what: "Space" | "computer") =>
    (e: unknown): never => {
      throw OLD_COMPUTER_REFUSAL.test(e instanceof Error ? e.message : String(e)) ? new NeedsUpdateError(what) : e;
    };

  async function tool(s: SpaceMachine, name: string, args: Record<string, unknown>): Promise<unknown> {
    if (!s.threadOnHost) throw new Error("This Space runs on its own, and its thread is not reachable from a phone yet.");
    // A Space shared with the account while its host is not: the relay grants that Space's thread alone.
    const granted = !hosts.has(s.hostId);
    const c = clientsFor(s.hostId, granted ? { "x-cua-thread-space": s.machine.id } : {});
    const r = await c.thread.callThreadTool({ space: idOf(s), name, argumentsJson: JSON.stringify(args) }).catch(needsUpdate("Space"));
    return toolJson(r.contentJson, r.isError);
  }

  /** A tool that runs on a machine's own daemon rather than in one Space's thread (`task_options`, `task_start` on a new Space). */
  async function hostTool(hostId: string, name: string, args: Record<string, unknown>): Promise<unknown> {
    const c = clientsFor(hostId);
    const r = await c.thread.callThreadTool({ space: `relay:${hostId}`, name, argumentsJson: JSON.stringify(args) }).catch(needsUpdate("computer"));
    return toolJson(r.contentJson, r.isError);
  }
  /** The computers that can answer a host tool now, the first online one first. */
  const onlineHosts = () => {
    const online = machinesKnown.filter((m) => m.online).map((m) => m.id);
    // Where a Space can be made follows what each member of the pool states it can host (the app core), not what kind of
    // thing it is: a computer that does not run Spaces for the account, or a phone that states nothing, is not offered.
    const offered = placementOffered(
      computersKnown.map((c) => ({ id: c.id, online: c.online, hostsSpaces: c.hostsSpaces, capabilities: c.capabilities })),
      {},
    );
    return offered ? online.filter((id) => offered.includes(id)) : online;
  };
  const tooOld = "That computer can't start tasks from a phone yet. Update Cua Spaces on it.";
  const missingTool = (e: unknown) => isMissingThreadTool(e) || (e instanceof Error && /can't be called from another device|older Cua/i.test(e.message));

  const capabilities: Capabilities = {
    sample: false,
    stream: "viewer",
    input: true,
    sleep: false,
    share: true,
    startTask: true,
    diff: true,
    scopes: true,
    steer: true,
    push: true,
    why: { sleep: "Putting a Space to sleep from the phone is not available yet." },
  };

  return {
    kind: "live",
    capabilities,
    async listSpaces(): Promise<SpaceList> {
      const token = await d.token();
      const session = await d.deviceSession();
      const rows = await d.relay.machines(token, session);
      // The account's phones are part of the pool; not being able to list them never hides the rest.
      const devices = await d.relay.devices(token, session).catch(() => []);
      // The pool, by the desktop's rules (the app core): a computer is never a Space.
      const view = directory({ machines: rows, devices, error: "", reading: false, now: now() });
      if (!view) throw new Error("This build cannot list your Spaces yet: the app core is not linked.");
      computersKnown = view.computers;
      const m = mapDirectory(rows, view.computers, view.spaces);
      machinesKnown = m.machines;
      spaces = new Map(m.spaces.map((s) => [idOf(s), s]));
      hosts = new Set(rows.map((r) => r.id));
      const inputs: SpaceInput[] = [];
      let missed = 0;
      // Bounded fan-out: a few Spaces at a time, so a long list does not open a hundred calls.
      const queue = [...m.spaces];
      const work = async () => {
        for (let s = queue.shift(); s; s = queue.shift()) {
          const machine = m.machines.find((x) => x.id === s.hostId) ?? {
            id: s.hostId,
            name: s.hostId,
            kind: "mac" as const,
            online: s.machine.online,
            lastSeenMs: null,
          };
          let members: AgentMember[] = [];
          let last: ThreadEntry | null = null;
          let unread = 0;
          if (s.machine.online || machine.online) {
            try {
              members = agentMembersFromTool(await tool(s, "agent_members", agentMembersArgs({ spaceId: idOf(s), overview: true }))).members;
              const p = threadPageFromTool(await tool(s, "thread_read", threadReadArgs({ spaceId: idOf(s), limit: 10 })));
              last = [...p.entries].reverse().find((e) => e.kind === "message" || e.kind === "approval") ?? null;
              unread = p.unread;
            } catch {
              missed++;
            }
          }
          inputs.push({
            id: idOf(s),
            name: s.machine.name || s.machine.id,
            machine: { ...machine, online: s.machine.online && machine.online },
            state: s.state,
            scope: s.scope,
            kindLabel: s.kindLabel,
            scene: "blank",
            role: s.role,
            members,
            people: [],
            last,
            unread,
            canSleep: false,
            canShare: s.role === "owner",
          });
        }
      };
      await Promise.all([work(), work(), work(), work()]);
      return { ...summarize(inputs, m.machines, true, missed > 0), computers: view.computers, words: { reachingLine: view.reachingLine, empty: view.empty } };
    },
    async readThread(spaceId, o): Promise<ThreadPage> {
      const s = spaceOf(spaceId);
      return threadPageFromTool(
        await tool(
          s,
          "thread_read",
          threadReadArgs({ spaceId, after: o.after, before: o.before, limit: o.limit, markRead: o.markRead, followMs: o.followMs, liveAfter: o.liveAfter }),
        ),
      );
    },
    async members(spaceId) {
      return agentMembersFromTool(await tool(spaceOf(spaceId), "agent_members", agentMembersArgs({ spaceId, overview: true }))).members;
    },
    async post(spaceId, text, o) {
      const s = spaceOf(spaceId);
      const agent = o?.agent?.replace(/^@/, "") || null;
      // A message to a running agent goes through `agent_message`: the machine records it in the thread, queues it behind the
      // turn (Queue) or interrupts the turn first (Steer), and takes the same client id twice as one message.
      if ((o?.mode === "steer" || o?.mode === "queue") && agent && o.clientId && o.replyTo == null && !o.view) {
        await tool(s, "agent_message", { name: agent, text, force: o.mode === "steer", client_id: o.clientId });
        return null;
      }
      // Otherwise Steer interrupts the running step first (best effort: a message that cannot interrupt still goes, and queues).
      if (o?.mode === "steer" && agent) await tool(s, "agent_interrupt", { name: agent }).catch(() => {});
      // The same client id again is the first entry (the daemon answers `replayed`); an older daemon ignores it, and the
      // outbox then finds a message that may have gone through by its author, exact text and time.
      return threadPostFromTool(await tool(s, "thread_post", threadPostArgs({ spaceId, text, replyTo: o?.replyTo, view: o?.view, clientId: o?.clientId })));
    },
    async answer(spaceId, approval, approve, o) {
      const note = approve ? (o?.scope === "space" ? "Allowed for this Space" : "Approved") : "Denied";
      return threadPostFromTool(
        await tool(
          spaceOf(spaceId),
          "thread_post",
          threadPostArgs({ spaceId, text: note, answer: { approval, approve, scope: o?.scope, clientId: o?.clientId } }),
        ),
      );
    },
    async toolRows(spaceId, o) {
      return toolRowsFromTool(await tool(spaceOf(spaceId), "thread_tool_rows", toolRowsArgs({ spaceId, cursor: o?.cursor, followMs: o?.followMs })));
    },
    async taskOptions() {
      const hosts = onlineHosts();
      if (!hosts.length) throw new Error("None of your computers is online and set up to run Spaces.");
      const offline = offlineMachineOptions(machinesKnown, new Set(hosts));
      const names = new Map(machinesKnown.map((m) => [m.id, m.name]));
      // Every online computer says what it can start and which agents are connected there.
      const answers = await Promise.allSettled(hosts.map(async (h) => taskOptionsFromTool(await hostTool(h, "task_options", {}))));
      const parts: TaskOptions[] = [];
      let first: unknown = null;
      answers.forEach((r, i) => {
        if (r.status === "fulfilled") return void parts.push(r.value);
        first ??= r.reason;
        // A computer that cannot answer (an older Cua Spaces) is listed, with why it cannot start one.
        const id = hosts[i]!;
        parts.push({
          machines: [
            {
              id,
              name: names.get(id) ?? id,
              online: true,
              canCreate: false,
              why: missingTool(r.reason) ? "Update Cua Spaces on it to start tasks from here" : errorText(r.reason),
              os: [],
              readyAgents: [],
            },
          ],
          agents: [],
        });
      });
      if (answers.every((r) => r.status === "rejected")) throw missingTool(first) ? new Error(tooOld) : first;
      const merged = mergeTaskOptions(parts, names);
      return { ...merged, machines: [...merged.machines, ...offline] };
    },
    async startTask(req: TaskRequest) {
      const w = req.where;
      try {
        let host: string | undefined;
        let r: unknown;
        if (w.kind === "space") {
          const s = spaceOf(w.space);
          host = s.hostId;
          r = await tool(s, "task_start", req as unknown as Record<string, unknown>);
        } else {
          host = w.kind === "machine" ? w.machine : onlineHosts()[0];
          if (!host) throw new Error("None of your computers is online and set up to run Spaces.");
          r = await hostTool(host, "task_start", req as unknown as Record<string, unknown>);
        }
        taskHosts.set(req.client_id, host);
        return taskResultFromTool(r);
      } catch (e) {
        throw missingTool(e) ? new Error(tooOld) : e;
      }
    },
    async taskStatus(clientId) {
      const host = taskHosts.get(clientId) ?? onlineHosts()[0];
      if (!host) throw new Error("None of your computers is online and set up to run Spaces.");
      return taskResultFromTool(await hostTool(host, "task_status", taskStatusArgs(clientId)));
    },
    async changes(spaceId, o) {
      return changesFromTool(await tool(spaceOf(spaceId), "space_changes", changesArgs({ spaceId, repo: o?.repo })));
    },
    async fileDiff(spaceId, path, o) {
      return fileDiffFromTool(await tool(spaceOf(spaceId), "space_file_diff", fileDiffArgs({ spaceId, path, repo: o?.repo })));
    },
    async filePreview(spaceId, path, o) {
      return filePreviewFromTool(await tool(spaceOf(spaceId), "space_file_preview", filePreviewArgs({ spaceId, path, repo: o?.repo, rev: o?.rev })));
    },
    push: {
      async status() {
        return d.relay.pushStatus(await d.token(), await d.deviceSession());
      },
      async register(body) {
        const id = d.deviceId?.();
        if (!id) throw new Error("This phone is not enrolled yet.");
        await d.relay.registerPush(await d.token(), id, body, await d.deviceSession());
      },
      async unregister() {
        const id = d.deviceId?.();
        if (!id) return;
        await d.relay.unregisterPush(await d.token(), id, await d.deviceSession());
      },
    },
    async removeAllPhones() {
      return (await d.relay.revokeAllPhones(await d.token(), await d.deviceSession())).length;
    },
    async streamTicket(spaceId): Promise<StreamTicket | null> {
      const s = spaceOf(spaceId);
      const c = clientsFor(s.machine.id);
      // A ticket for looking: view only (asked for by name), short. Touching the desktop is not a ticket's to give.
      const r = await c.system
        .createViewerTicket({ ttl: { seconds: BigInt(STREAM_TICKET_SECONDS), nanos: 0 }, policy: SessionPolicy.VIEW_ONLY })
        .catch(needsUpdate("Space"));
      // The page gets the ticket in its address and nothing else: never the account's bearer or this phone's session.
      return { viewerUrl: `${d.relay.machineUrl(s.machine.id)}${r.viewerPath}`, expiresAtMs: now() + STREAM_TICKET_SECONDS * 1000, width: null, height: null };
    },
    async sendInput(spaceId, a: InputAction) {
      try {
        const s = spaceOf(spaceId);
        const c = clientsFor(s.machine.id).computer;
        const target = { space: CoordinateSpace.NORMALIZED };
        switch (a.type) {
          case "click":
            await c.pointer({
              target,
              action: {
                case: "click",
                value: {
                  position: { x: a.x, y: a.y },
                  button: a.button === "right" ? MouseButton.RIGHT : MouseButton.LEFT,
                  count: a.button === "double" ? 2 : 1,
                },
              },
            });
            return;
          case "scroll":
            await c.pointer({
              target,
              action: { case: "scroll", value: { position: { x: a.x, y: a.y }, deltaX: a.dx, deltaY: a.dy, unit: ScrollUnit.PIXEL } },
            });
            return;
          case "drag":
            await c.pointer({ target, action: { case: "drag", value: { from: a.from, to: a.to, button: MouseButton.LEFT } } });
            return;
          case "type":
            await c.keyboard({ action: { case: "type", value: { text: a.text } } });
            return;
          case "key": {
            const named = KEYS[a.key];
            if (named === undefined) return;
            await c.keyboard({
              action: {
                case: "press",
                value: {
                  key: create(KeyInputSchema, { key: { case: "named", value: named } }),
                  modifiers: (a.modifiers ?? []).flatMap((m) => (KEYS[m] !== undefined ? [KEYS[m]!] : [])),
                },
              },
            });
          }
        }
      } catch (e) {
        // The computer's own words for a share that only lets this account look, or a desktop that is not shared:
        // a state, not a failure.
        if (/view-only share|does not share its desktop/i.test(e instanceof Error ? e.message : String(e))) throw new ControlNotAllowedError();
        throw e;
      }
    },
    async takeControl(spaceId, on) {
      // Taking control interrupts every working agent in the room (`agent_interrupt` is one of the member tools a
      // host serves to phones; pause and resume are not). Giving back needs no call: an agent wakes on its next message.
      // Touching the desktop does not depend on the thread: a Space without one has no agents to pause, and a refused
      // or failed pause must not keep the person from taking the desktop.
      if (!on) return;
      const s = spaceOf(spaceId);
      try {
        const members = agentMembersFromTool(await tool(s, "agent_members", threadMembersArgs({ spaceId }))).members;
        for (const m of members) if (m.status === "working") await tool(s, "agent_interrupt", { name: m.name });
      } catch {
        /* nothing to pause here */
      }
    },
    async stopAgent(spaceId, handle) {
      if (handle) await tool(spaceOf(spaceId), "agent_interrupt", { name: handle });
    },
    async sleep() {
      throw new Error(capabilities.why.sleep);
    },
    async share(spaceId, email, role) {
      const s = spaceOf(spaceId);
      const token = await d.token();
      const sess = await d.deviceSession();
      const row = (await d.relay.machines(token, sess)).find((m) => m.id === s.machine.id);
      const allow = new Set(row?.allow ?? []);
      const viewers = new Set(row?.viewers ?? []);
      if (role === "editor") {
        allow.add(email);
        viewers.delete(email);
      } else {
        viewers.add(email);
        allow.delete(email);
      }
      await d.relay.patchMachine(token, s.machine.id, { allow: [...allow], viewers: [...viewers] }, sess);
    },
  };
}
