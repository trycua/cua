// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createLiveBackend } from "../src/data/live/backend";
import { ControlNotAllowedError, NeedsUpdateError } from "../src/data/types";
import { RelayClient } from "../src/data/live/relay";
import { installFixtureCore, uninstallCore } from "./helpers/core";
import { fakeRelay } from "./helpers/fakeDaemon";

const machines = [
  {
    id: "studio",
    name: "Studio Mac",
    owner: { id: "acct:you" },
    role: "owner",
    online: true,
    sharing: true,
    version: "1",
    url: "",
    meta: { "cua.host.spaces": "on", "cua.os": "macos" },
  },
  {
    id: "space-aaaa1111",
    name: "checkout-api",
    owner: { id: "acct:you" },
    role: "owner",
    online: true,
    sharing: true,
    version: "1",
    url: "",
    host: "studio",
    meta: { "cua.os": "macos" },
  },
];

const entry = (seq: number, text: string, author = { kind: "person", id: "acct:sam", name: "Sam" }) => ({
  seq,
  id: `e${seq}`,
  ts_ms: 1_000 * seq,
  author,
  kind: "message",
  text,
  mentions: [],
  reply_to: null,
  attachments: [],
});

function setup(
  extra: {
    touchRefused?: string;
    threadRefused?: string;
    tools?: (n: string, a: Record<string, unknown>) => unknown;
    isError?: (n: string) => boolean;
    rest?: Parameters<typeof fakeRelay>[0]["rest"];
  } = {},
) {
  const fake = fakeRelay({
    machines,
    tools: (name, args) => {
      if (extra.tools) return extra.tools(name, args);
      if (name === "agent_members")
        return { members: [{ name: "claude", harness: "claude-code", status: { state: "working", activity: "running tests" }, sessions: 1 }] };
      if (name === "thread_read")
        return {
          entries: [entry(1, "Hello"), entry(2, "Fixed: 149 passed.", { kind: "agent", id: "agent:claude", name: "claude" })],
          cursor: 2,
          unread: 1,
          me: { id: "acct:you" },
          role: "owner",
          live: { key: "", agents: [] },
        };
      if (name === "thread_post") return { entry: entry(3, String(args.text), { kind: "person", id: "acct:you", name: "You" }) };
      return {};
    },
    isError: extra.isError,
    rest: extra.rest,
    touchRefused: extra.touchRefused,
    threadRefused: extra.threadRefused,
  });
  const backend = createLiveBackend({
    relay: new RelayClient("https://relay.test", fake.fetcher as never),
    token: async () => "acct-token",
    deviceSession: async () => "dev-session",
    me: () => "acct:you",
    deviceId: () => "dev_0123456789abcdef01234567",
    fetcher: fake.fetcher as never,
    now: () => 2_000_000,
  });
  return { fake, backend };
}

beforeAll(installFixtureCore);
afterAll(uninstallCore);

describe("the live backend against a fake relay and a fake host daemon", () => {
  it("lists Spaces under their machine with the thread's members and last message", async () => {
    const { backend, fake } = setup();
    const list = await backend.listSpaces();
    expect(list.machines.map((m) => m.name)).toEqual(["Studio Mac"]);
    expect(list.spaces).toHaveLength(1);
    expect(list.spaces[0]).toMatchObject({
      name: "checkout-api",
      machineName: "Studio Mac",
      statusWord: "Running",
      unread: 1,
      lastMessage: { text: "Fixed: 149 passed." },
    });
    const call = fake.seen.find((s) => s.tool?.name === "thread_read")!;
    // The thread runs on the host that provides the Space, named by its relay id, as the account.
    expect(call.url).toBe("https://relay.test/m/studio/cua.env.v1.HostThreadService/CallThreadTool");
    expect(call.tool!.space).toBe("relay:space-aaaa1111");
    expect(call.headers.authorization).toBe("Bearer acct-token");
    expect(call.headers["x-cua-device-session"]).toBe("dev-session");
  });

  it("posts and answers through thread_post", async () => {
    const { backend, fake } = setup();
    await backend.listSpaces();
    const e = await backend.post("relay:space-aaaa1111", "@claude go", { replyTo: 2 });
    expect(e?.text).toBe("@claude go");
    expect(fake.seen.at(-1)!.tool).toMatchObject({ name: "thread_post", args: { space: "relay:space-aaaa1111", text: "@claude go", reply_to: 2 } });
    await backend.answer("relay:space-aaaa1111", 7, true);
    expect(fake.seen.at(-1)!.tool!.args).toMatchObject({ answer: { approval: 7, approve: true } });
  });

  it("turns a daemon refusal into an error with its words", async () => {
    const { backend } = setup({ isError: (n) => n === "thread_post", tools: () => "You can watch this Space only." });
    await backend.listSpaces().catch(() => {});
    await expect(backend.post("relay:space-aaaa1111", "hi")).rejects.toThrow("You can watch this Space only.");
  });

  it("mints a viewer ticket on the Space's own machine and builds the page URL", async () => {
    const { backend, fake } = setup();
    await backend.listSpaces();
    const t = await backend.streamTicket("relay:space-aaaa1111");
    expect(t?.viewerUrl).toBe("https://relay.test/m/space-aaaa1111/viewer/#ticket=tkt");
    // The page holds the ticket in its address and nothing of the account's: no header rides with it.
    expect(t).not.toHaveProperty("headers");
    expect(JSON.stringify(t)).not.toMatch(/acct-token|dev-session|Bearer/);
    // A ticket for looking, asked for by name, and short.
    expect(fake.seen.at(-1)!.ticketPolicy).toBe(1);
    expect(fake.seen.at(-1)!.ticketTtl).toBe(300);
    expect(t!.expiresAtMs).toBe(2_000_000 + 300_000); // the test's clock plus the ticket's life
  });

  it("sends touch as normalized pointer and keyboard operations", async () => {
    const { backend, fake } = setup();
    await backend.listSpaces();
    await backend.sendInput("relay:space-aaaa1111", { type: "click", x: 0.25, y: 0.5, button: "right" });
    const p = fake.seen.at(-1)!.pointer!;
    expect(p.target?.space).toBe(fake.NORMALIZED);
    expect(p.action).toMatchObject({ case: "click", value: { position: { x: 0.25, y: 0.5 }, button: 2 } });
    await backend.sendInput("relay:space-aaaa1111", { type: "scroll", x: 0.5, y: 0.5, dx: 0, dy: 12 });
    expect(fake.seen.at(-1)!.pointer!.action).toMatchObject({ case: "scroll", value: { deltaY: 12 } });
    await backend.sendInput("relay:space-aaaa1111", { type: "type", text: "ls" });
    expect(fake.seen.at(-1)!.keyboard!.action).toMatchObject({ case: "type", value: { text: "ls" } });
    await backend.sendInput("relay:space-aaaa1111", { type: "key", key: "escape", modifiers: ["cmd"] });
    expect(fake.seen.at(-1)!.keyboard!.action.case).toBe("press");
  });

  it("says a Space needs an update when its computer predates phones, and passes other refusals on", async () => {
    const old = "this computer needs the latest Cua Spaces to be used from a phone: update it";
    const { backend } = setup({ threadRefused: old });
    await backend.listSpaces().catch(() => {});
    const e = await backend.post("relay:space-aaaa1111", "hi").then(
      () => null,
      (err: unknown) => err,
    );
    expect(e).toBeInstanceOf(NeedsUpdateError);
    expect((e as Error).message).toBe("This Space needs an update. Update Cua Spaces on the computer it runs on, then try again.");
    // Any other refusal keeps its own words.
    const other = setup({ threadRefused: "this phone is not on the list" });
    await other.backend.listSpaces().catch(() => {});
    await expect(other.backend.post("relay:space-aaaa1111", "hi")).rejects.toThrow(/this phone is not on the list/);
  });

  it("removes every phone of the account with one call, under this phone's session", async () => {
    const calls: string[] = [];
    const { backend, fake } = setup({
      rest: (method, path) => {
        calls.push(`${method} ${path}`);
        return method === "POST" && path === "/v1/devices/revoke-phones" ? { body: { revoked: ["dev_a", "dev_b"] } } : undefined;
      },
    });
    expect(await backend.removeAllPhones!()).toBe(2);
    expect(calls).toContain("POST /v1/devices/revoke-phones");
    const seen = fake.seen.find((x) => x.url.endsWith("/v1/devices/revoke-phones"))!;
    expect(seen.headers["x-cua-device-session"]).toBe("dev-session");
    expect(seen.headers.authorization).toBe("Bearer acct-token");
  });

  it("says plainly when the share only lets this account look, or the desktop is not shared", async () => {
    for (const words of [
      "view-only share: ada@example.test cannot call /cua.env.v1.ComputerService/Pointer",
      "this computer does not share its desktop (it only provides Spaces): ada cannot call /cua.env.v1.ComputerService/Pointer",
    ]) {
      const { backend } = setup({ touchRefused: words });
      await backend.listSpaces();
      await expect(backend.sendInput("relay:space-aaaa1111", { type: "click", x: 0.5, y: 0.5 })).rejects.toThrow(ControlNotAllowedError);
      await expect(backend.sendInput("relay:space-aaaa1111", { type: "type", text: "x" })).rejects.toThrow(
        "You can only look at this desktop. Its owner can let you control it in the share.",
      );
    }
  });

  it("taking control interrupts the agents that are working", async () => {
    const { backend, fake } = setup();
    await backend.listSpaces();
    await backend.takeControl("relay:space-aaaa1111", true);
    expect(fake.seen.at(-1)!.tool).toMatchObject({ name: "agent_interrupt", args: { name: "claude" } });
  });

  it("taking control still works in a Space that has no thread to pause", async () => {
    const { backend } = setup({ isError: (n) => n === "agent_members" });
    await backend.listSpaces();
    await expect(backend.takeControl("relay:space-aaaa1111", true)).resolves.toBeUndefined();
  });

  it("does not offer what the daemon cannot do from a phone", () => {
    const { backend } = setup();
    expect(backend.capabilities.sleep).toBe(false);
    expect(backend.capabilities.why.sleep).toMatch(/not available yet/);
  });
});

describe("what the phone needs beyond the thread, against a fake relay and a fake host daemon", () => {
  const SPACE = "relay:space-aaaa1111";

  it("asks every online machine what it can start, merges the answers, and says an old daemon cannot", async () => {
    const { backend, fake } = setup({
      tools: (name) =>
        name === "task_options"
          ? {
              machine: { id: "studio", os: ["macos"], online: true, can_create: true },
              agents: [
                { harness: "claude-code", label: "Claude Code", ready: true },
                { harness: "openai-codex", label: "Codex", ready: false, why: "Sign in" },
              ],
            }
          : name === "agent_members"
            ? { members: [] }
            : name === "thread_read"
              ? { entries: [], cursor: 0, me: { id: "acct:you" } }
              : {},
    });
    await backend.listSpaces();
    const o = await backend.taskOptions!();
    expect(o.machines).toEqual([{ id: "studio", name: "Studio Mac", online: true, canCreate: true, why: null, os: ["macos"], readyAgents: ["claude-code"] }]);
    expect(o.agents.map((a) => [a.harness, a.ready])).toEqual([
      ["claude-code", true],
      ["openai-codex", false],
    ]);
    const call = fake.seen.find((s) => s.tool?.name === "task_options")!;
    expect(call.url).toBe("https://relay.test/m/studio/cua.env.v1.HostThreadService/CallThreadTool");
    expect(call.tool!.space).toBe("relay:studio");
    const old = setup({
      isError: (n) => n === "task_options",
      tools: (name) =>
        name === "task_options" ? "task_options can't be called from another device" : name === "agent_members" ? { members: [] } : { entries: [], cursor: 0 },
    });
    await old.backend.listSpaces();
    await expect(old.backend.taskOptions!()).rejects.toThrow(/Update Cua Spaces on it/);
  });

  it("starts a task on a machine with the request the core built, or in a Space that exists, with the client id", async () => {
    const { backend, fake } = setup({
      tools: (name, args) =>
        name === "task_start"
          ? { space: args.where && (args.where as { kind: string }).kind === "space" ? SPACE : "relay:space-new1", created: true, entry_seq: 4, state: "ready" }
          : name === "task_status"
            ? { client_id: args.client_id, state: "creating", space: null }
            : name === "agent_members"
              ? { members: [] }
              : { entries: [], cursor: 0 },
    });
    await backend.listSpaces();
    const req = {
      client_id: "task-0001",
      agent: "claude-code",
      prompt: "fix it",
      message: "@claude fix it",
      where: { kind: "machine" as const, machine: "studio" },
      os: "linux",
    };
    const r = await backend.startTask!(req);
    expect(r).toMatchObject({ space: "relay:space-new1", created: true, entrySeq: 4 });
    const call = fake.seen.filter((s) => s.tool?.name === "task_start").at(-1)!;
    expect(call.tool!.space).toBe("relay:studio");
    expect(call.tool!.args).toEqual(req);
    await backend.startTask!({ ...req, client_id: "task-0002", where: { kind: "space", space: SPACE } });
    const inSpace = fake.seen.filter((s) => s.tool?.name === "task_start").at(-1)!;
    expect(inSpace.tool!.space).toBe(SPACE);
    // Auto goes to the first online machine's daemon, which places it.
    await backend.startTask!({ ...req, client_id: "task-0003", where: { kind: "auto" } });
    expect(fake.seen.filter((s) => s.tool?.name === "task_start").at(-1)!.tool!.space).toBe("relay:studio");
    // A task still being made is followed on the machine that took it.
    const status = await backend.taskStatus!("task-0001");
    expect(status).toMatchObject({ state: "creating", space: "" });
    expect(fake.seen.at(-1)!.tool).toMatchObject({ name: "task_status", space: "relay:studio", args: { client_id: "task-0001" } });
  });

  it("sends a message to a running agent through agent_message: Queue waits behind the turn, Steer interrupts it, both replay-safe", async () => {
    const { backend, fake } = setup();
    await backend.listSpaces();
    expect(await backend.post(SPACE, "@claude stop that, do this", { clientId: "c1", mode: "steer", agent: "claude" })).toBeNull();
    expect(fake.seen.at(-1)!.tool).toMatchObject({
      name: "agent_message",
      space: SPACE,
      args: { name: "claude", text: "@claude stop that, do this", force: true, client_id: "c1" },
    });
    await backend.post(SPACE, "@claude then this", { clientId: "c2", mode: "queue", agent: "@claude" });
    expect(fake.seen.at(-1)!.tool).toMatchObject({ name: "agent_message", args: { name: "claude", force: false, client_id: "c2" } });
  });

  it("falls back to the thread's own post (with an interrupt first for Steer) when the message replies or shows the screen, and a plain message carries its client id", async () => {
    const { backend, fake } = setup();
    await backend.listSpaces();
    await backend.post(SPACE, "@claude look at this", { clientId: "c5", mode: "steer", agent: "claude", replyTo: 3 });
    const tools = fake.seen
      .map((s) => s.tool)
      .filter(Boolean)
      .slice(-2);
    expect(tools.map((t) => t!.name)).toEqual(["agent_interrupt", "thread_post"]);
    expect(tools[1]!.args).toMatchObject({ reply_to: 3 });
    await backend.post(SPACE, "hello", { clientId: "c6" });
    expect(fake.seen.at(-1)!.tool).toMatchObject({ name: "thread_post", args: { text: "hello", client_id: "c6" } });
  });

  it("answers with a scope and a client id inside the answer", async () => {
    const { backend, fake } = setup();
    await backend.listSpaces();
    await backend.answer(SPACE, 8, true, { scope: "space", clientId: "c3" });
    expect(fake.seen.at(-1)!.tool!.args.answer).toEqual({ approval: 8, approve: true, scope: "space", client_id: "c3" });
    await backend.answer(SPACE, 8, false, { scope: "space", clientId: "c4" });
    expect(fake.seen.at(-1)!.tool!.args.answer).toEqual({ approval: 8, approve: false, client_id: "c4" });
  });

  it("reads tool rows, changes, a file's diff and a preview from the Space's host", async () => {
    const { backend, fake } = setup({
      tools: (name) => {
        if (name === "agent_members") return { members: [] };
        if (name === "thread_read") return { entries: [], cursor: 0, me: { id: "acct:you" } };
        if (name === "thread_tool_rows")
          return { groups: [{ after_seq: 2, agent: "claude", calls: [{ id: "a", cat: "ran", line: "Ran ls", state: "done" }] }], cursor: 5, caught_up: true };
        if (name === "space_changes")
          return { repo: "a/b", branch: "x", base: "main", files: [{ path: "a.ts", status: "modified", additions: 1, deletions: 1 }] };
        if (name === "space_file_diff") return { path: "a.ts", patch: "@@ -1 +1 @@\n-a\n+b\n" };
        if (name === "space_file_preview") return { path: "a.ts", mime: "text/plain", size: 3, encoding: "utf8", content: "abc" };
        return {};
      },
    });
    await backend.listSpaces();
    expect((await backend.toolRows!(SPACE, { cursor: 4, followMs: 15000 })).groups[0]!.calls[0]!.line).toBe("Ran ls");
    expect(fake.seen.at(-1)!.tool).toMatchObject({ name: "thread_tool_rows", args: { space: SPACE, cursor: 4, follow_ms: 15000 } });
    expect((await backend.changes!(SPACE)).files[0]!.path).toBe("a.ts");
    expect((await backend.fileDiff!(SPACE, "a.ts")).patch).toContain("+b");
    expect(fake.seen.at(-1)!.tool).toMatchObject({ name: "space_file_diff", args: { space: SPACE, path: "a.ts" } });
    expect((await backend.filePreview!(SPACE, "a.ts", { rev: "head" })).content).toBe("abc");
    expect(fake.seen.at(-1)!.tool!.args).toMatchObject({ rev: "head" });
  });

  it("registers this phone's push token with the relay under its device id, and removes it", async () => {
    const { backend, fake } = setup({
      rest: (method, path) => (path.startsWith("/v1/push/registrations/") ? { status: method === "DELETE" ? 204 : 204 } : undefined),
    });
    await backend.push!.register({ platform: "ios", token: "ab".repeat(32) });
    const put = fake.seen.at(-1)!;
    expect(put.method).toBe("PUT");
    expect(put.url).toBe("https://relay.test/v1/push/registrations/dev_0123456789abcdef01234567");
    expect(put.restBody).toMatchObject({ platform: "ios" });
    expect(put.headers.authorization).toBe("Bearer acct-token");
    await backend.push!.unregister();
    expect(fake.seen.at(-1)!.method).toBe("DELETE");
  });
});
