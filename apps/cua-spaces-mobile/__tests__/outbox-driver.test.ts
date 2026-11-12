// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Backend, ThreadEntry } from "../src/data/types";
import { OutboxDriver } from "../src/store/outbox";
import { isNetworkError } from "../src/store/network";
import { installFixtureCore, uninstallCore } from "./helpers/core";
import { memoryKv, testStore } from "./helpers/store";

beforeAll(installFixtureCore);
afterAll(uninstallCore);
beforeEach(() => jest.useFakeTimers());
afterEach(() => jest.useRealTimers());

const entry = (seq: number, text: string, clientId?: string): ThreadEntry =>
  ({
    seq,
    id: `e${seq}`,
    tsMs: 1_000,
    author: { kind: "person", id: "acct:you", name: "You" },
    kind: "message",
    text,
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
    ...(clientId ? { clientId } : {}),
  }) as ThreadEntry;

/** A backend that records what the outbox asks of it. */
function fakeBackend(over: Partial<Backend> = {}) {
  const posts: { text: string; clientId?: string | null }[] = [];
  const b = {
    kind: "demo",
    capabilities: { sample: true, why: {} },
    post: jest.fn(async (_s: string, text: string, o?: { clientId?: string | null }) => {
      posts.push({ text, clientId: o?.clientId });
      return entry(posts.length, text, o?.clientId ?? undefined);
    }),
    answer: jest.fn(async () => entry(99, "Approved")),
    readThread: jest.fn(async () => ({
      entries: [],
      live: { key: "", agents: [] },
      cursor: 0,
      caughtUp: true,
      unread: 0,
      readSeq: 0,
      me: "acct:you",
      hasMoreBefore: false,
      role: "owner",
    })),
    ...over,
  } as unknown as Backend;
  return { b, posts };
}

const ids = () => {
  let n = 0;
  return () => `m${++n}`;
};
const clock = () => {
  let t = 1_000_000;
  return { now: () => t, advance: (ms: number) => void (t += ms) };
};
const flush = async () => {
  for (let i = 0; i < 10; i++) await Promise.resolve();
};

function driver(b: Backend, store = testStore(), c = clock()) {
  return { d: new OutboxDriver({ store, backend: () => b, me: () => "acct:you", now: c.now, newId: ids() }), c, store };
}

describe("the outbox driver (the core's rules, run against a backend)", () => {
  it("writes a message down, posts it once with its id as the client id, and says it was delivered", async () => {
    const { b, posts } = fakeBackend();
    const { d } = driver(b);
    const sent = await d.send({ space: "relay:a", text: "hello" });
    const delivered = await sent.done;
    expect(posts).toEqual([{ text: "hello", clientId: "m1" }]);
    expect(delivered.entry?.text).toBe("hello");
    expect(d.view("relay:a").rows).toEqual([]);
  });

  it("a lost connection keeps the message, says so honestly, and delivers it once when the connection is back", async () => {
    let up = false;
    const { b, posts } = fakeBackend({
      post: jest.fn(async (_s: string, text: string, o?: { clientId?: string | null }) => {
        if (!up) throw new TypeError("Network request failed");
        posts.push({ text, clientId: o?.clientId });
        return entry(1, text);
      }) as never,
    });
    const { d } = driver(b);
    const sent = await d.send({ space: "relay:a", text: "later" });
    await flush();
    const row = d.view("relay:a").rows[0]!;
    expect(row).toMatchObject({ state: "waiting", label: "Waiting for a connection" });
    expect(d.view().banner).toBe("You're offline. 1 message will send when you're back.");
    up = true;
    d.setOnline(true);
    await sent.done;
    expect(posts).toEqual([{ text: "later", clientId: "m1" }]);
    expect(d.view("relay:a").rows).toEqual([]);
  });

  it("keeps the order of a Space: the second message waits for the first", async () => {
    const seen: string[] = [];
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const { b } = fakeBackend({
      post: jest.fn(async (_s: string, text: string) => {
        seen.push(text);
        if (text === "one") await gate;
        return entry(seen.length, text);
      }) as never,
    });
    const { d } = driver(b);
    const one = await d.send({ space: "relay:a", text: "one" });
    const two = await d.send({ space: "relay:a", text: "two" });
    await flush();
    expect(seen).toEqual(["one"]);
    release();
    await Promise.all([one.done, two.done]);
    expect(seen).toEqual(["one", "two"]);
  });

  it("an app that quit while posting looks in the thread first: found means delivered and nothing is posted again", async () => {
    const kv = memoryKv();
    const first = fakeBackend({
      post: jest.fn(() => new Promise(() => {})) as never, // the app quit before this answered
    });
    const a = driver(first.b, testStore(kv));
    await a.d.send({ space: "relay:a", text: "was posting" });
    await flush();
    await a.store.get("outbox"); // written
    // The next launch: the daemon has the message under its client id.
    const second = fakeBackend({
      readThread: jest.fn(async () => ({
        entries: [entry(5, "was posting", "m1")],
        live: { key: "", agents: [] },
        cursor: 5,
        caughtUp: true,
        unread: 0,
        readSeq: 5,
        me: "acct:you",
        hasMoreBefore: false,
        role: "owner",
      })) as never,
    });
    const b2 = driver(second.b, testStore(kv));
    await b2.d.ready;
    await flush();
    expect(second.b.readThread).toHaveBeenCalled();
    expect(second.posts).toEqual([]);
    expect(b2.d.view().rows).toEqual([]);
  });

  it("a claimed message the thread does not have goes again, once", async () => {
    const kv = memoryKv();
    const first = fakeBackend({ post: jest.fn(() => new Promise(() => {})) as never });
    const a = driver(first.b, testStore(kv));
    await a.d.send({ space: "relay:a", text: "never arrived" });
    await flush();
    const second = fakeBackend();
    const b2 = driver(second.b, testStore(kv));
    await b2.d.ready;
    await flush();
    expect(second.posts).toEqual([{ text: "never arrived", clientId: "m1" }]);
  });

  it("gives up loudly after five real refusals, holds what is behind it, and goes on when the person deletes or retries it", async () => {
    let refuse = true;
    const { b, posts } = fakeBackend({
      post: jest.fn(async (_s: string, text: string, o?: { clientId?: string | null }) => {
        if (refuse && text === "first") throw new Error("You can watch this Space only.");
        posts.push({ text, clientId: o?.clientId });
        return entry(posts.length, text);
      }) as never,
    });
    const c = clock();
    const { d } = driver(b, testStore(), c);
    const first = await d.send({ space: "relay:a", text: "first" });
    const second = await d.send({ space: "relay:a", text: "second" });
    first.done.catch(() => {});
    for (let i = 0; i < 6; i++) {
      await flush();
      c.advance(70_000);
      d.tick();
    }
    await flush();
    const v = d.view("relay:a");
    expect(v.rows.map((r) => r.state)).toEqual(["failed", "waiting"]);
    expect(v.rows[0]!.label).toBe("Not sent");
    expect(v.rows[1]!.label).toBe("Waiting for the message above");
    expect(v.banner).toBe("1 message did not send.");
    await expect(first.done).rejects.toThrow("You can watch this Space only.");
    refuse = false;
    d.retry(first.id);
    await Promise.all([second.done]);
    expect(posts.map((p) => p.text)).toEqual(["first", "second"]);
  });

  it("starts a task through the daemon with the request the core built, and hands back where it went", async () => {
    const startTask = jest.fn(async () => ({ space: "relay:new-1", created: true, entrySeq: 1, state: "ready" as const, pendingId: null }));
    const { b } = fakeBackend({ startTask: startTask as never });
    const { d } = driver(b);
    const request = { client_id: "task-1", agent: "claude-code", prompt: "fix it", message: "@claude fix it", where: { kind: "auto" }, os: "linux" };
    const sent = await d.send({ space: "task:task-1", kind: "task", text: "fix it", extra: { request } });
    const r = await sent.done;
    expect(startTask).toHaveBeenCalledWith(request);
    expect(r.task?.space).toBe("relay:new-1");
  });

  it("without the app core there is no queue to fake: the send goes straight through and a failure is the caller's", async () => {
    uninstallCore();
    try {
      const { b, posts } = fakeBackend();
      const { d } = driver(b);
      const sent = await d.send({ space: "relay:a", text: "direct" });
      await sent.done;
      expect(posts).toHaveLength(1);
      expect(d.view().rows).toEqual([]);
    } finally {
      installFixtureCore();
    }
  });
});

describe("what counts as the connection's fault", () => {
  it("is a lost connection or a timeout, not a refusal", () => {
    expect(isNetworkError(new TypeError("Network request failed"))).toBe(true);
    expect(isNetworkError(Object.assign(new Error("x"), { name: "ConnectError", code: 14 }))).toBe(true);
    expect(isNetworkError(new Error("The request timed out"))).toBe(true);
    expect(isNetworkError(Object.assign(new Error("bad gateway"), { name: "RelayError", status: 502 }))).toBe(true);
    expect(isNetworkError(new Error("You can watch this Space only."))).toBe(false);
    expect(isNetworkError(Object.assign(new Error("denied"), { name: "ConnectError", code: 7 }))).toBe(false);
    expect(isNetworkError(undefined)).toBe(false);
  });
});
