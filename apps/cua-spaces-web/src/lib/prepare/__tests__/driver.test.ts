// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { PrepareMessage, PrepareState } from "@/bridge";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";

import { isNetworkFailure, PrepareDriver, type PrepareEnv, type PrepareStorage } from "../driver";

/** A host that does what the test says: a create that can drop, an agent, a thread. */
function fakeHost() {
  const calls = { create: 0, attach: 0, post: [] as string[], verify: 0 };
  const thread: string[] = [];
  let creates: ((id: string) => Promise<string>)[] = [];
  const host = {
    calls,
    thread,
    /** The next creates, in order: each resolves with an id or rejects. */
    nextCreates(...fns: ((id: string) => Promise<string>)[]) {
      creates = fns;
    },
    online: true,
    home: null as { id: string; running: boolean; creating?: boolean } | null,
    follows: 0,
    followResult: null as Error | null,
    exists: true as boolean | null,
    postFails: false,
    hold: null as Promise<void> | null,
    env(): PrepareEnv {
      return {
        now: () => Date.now(),
        online: () => host.online,
        newPendingId: () => "pending:p1",
        findHome: () => host.home,
        startHome: async () => {},
        followCreate: async () => {
          host.follows += 1;
          if (host.followResult) throw host.followResult;
        },
        createHome: async () => {
          calls.create += 1;
          const f = creates.shift() ?? (async () => "local:home-1");
          return f("pending:p1");
        },
        attachAgent: async () => {
          calls.attach += 1;
        },
        post: async ({ message }: { spaceId: string; message: PrepareMessage }) => {
          if (host.hold) await host.hold;
          if (host.postFails) throw new Error("the Space is not reachable");
          calls.post.push(message.id);
          thread.push(message.text);
        },
        threadHas: async ({ message }) => {
          calls.verify += 1;
          return thread.includes(message.text);
        },
        agentReplied: async () => false,
        homeExists: () => host.exists,
      };
    },
  };
  return host;
}

function memoryStorage(): PrepareStorage & { saved: PrepareState | null } {
  const s: PrepareStorage & { saved: PrepareState | null } = {
    saved: null,
    load: () => (s.saved ? (JSON.parse(JSON.stringify(s.saved)) as PrepareState) : null),
    save: (state) => {
      s.saved = JSON.parse(JSON.stringify(state)) as PrepareState;
    },
  };
  return s;
}

const flush = () => vi.advanceTimersByTimeAsync(0);

describe.skipIf(!wasmBuilt)("the preparing driver (wasm core)", () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    vi.setSystemTime(1_800_000_000_000);
  });
  afterEach(() => vi.useRealTimers());

  it("prepares in order and delivers a message sent meanwhile, once", async () => {
    const core = await testCore();
    const host = fakeHost();
    let releaseCreate: (id: string) => void = () => {};
    host.nextCreates(() => new Promise<string>((r) => (releaseCreate = r)));
    const d = new PrepareDriver(core, host.env(), memoryStorage());
    d.start("claude-code");
    await flush();
    expect(host.calls.create).toBe(1);
    expect(d.view()?.phase).toBe("preparing");
    // The person types while the home is made: kept, nothing posted.
    d.send({ id: "m1", text: "fix the failing tests", agent: "claude-code" });
    await flush();
    expect(host.calls.post).toEqual([]);
    expect(d.view()?.queued.map((q) => q.text)).toEqual(["fix the failing tests"]);
    releaseCreate("local:home-1");
    await flush();
    await flush();
    expect(host.calls.attach).toBe(1);
    expect(host.calls.post).toEqual(["m1"]);
    expect(d.view()?.phase).toBe("ready");
    expect(d.state?.messages[0]?.state).toBe("delivered");
    // More ticks and a second ready change nothing.
    d.tick();
    await flush();
    expect(host.calls.post).toEqual(["m1"]);
  });

  it("a connection that drops twice waits, keeps the message, and carries on by itself", async () => {
    const core = await testCore();
    const host = fakeHost();
    host.nextCreates(
      async () => Promise.reject(new Error("connection reset by peer")),
      async () => Promise.reject(new Error("download the built-in Linux runtime: timed out")),
      async () => "local:home-1",
    );
    const d = new PrepareDriver(core, host.env(), memoryStorage());
    d.start("claude-code");
    d.send({ id: "m1", text: "hello", agent: "claude-code" });
    await flush();
    expect(d.view()?.phase).toBe("waiting");
    expect(d.view()?.line).toContain("connection dropped");
    expect(host.calls.create).toBe(1);
    // The first wait is 2 s ... the second 4 s.
    await vi.advanceTimersByTimeAsync(2_000);
    expect(host.calls.create).toBe(2);
    expect(d.view()?.phase).toBe("waiting");
    await vi.advanceTimersByTimeAsync(4_000);
    await flush();
    expect(host.calls.create).toBe(3);
    expect(d.view()?.phase).toBe("ready");
    expect(host.calls.post).toEqual(["m1"]);
  });

  it("offline waits without retrying and comes back by itself when the connection does", async () => {
    const core = await testCore();
    const host = fakeHost();
    host.online = false;
    host.nextCreates(
      async () => Promise.reject(new Error("dns error: failed to lookup address")),
      async () => "local:home-1",
    );
    const d = new PrepareDriver(core, host.env(), memoryStorage());
    d.start("claude-code");
    await flush();
    expect(d.view()?.phase).toBe("waiting");
    expect(d.view()?.line).toMatch(/^No internet connection/);
    await vi.advanceTimersByTimeAsync(120_000);
    expect(host.calls.create).toBe(1);
    host.online = true;
    d.online(true);
    await flush();
    await flush();
    expect(host.calls.create).toBe(2);
    expect(d.view()?.phase).toBe("ready");
  });

  it("a quit mid-prepare resumes from the saved record: the create runs again, the message is sent once", async () => {
    const core = await testCore();
    const storage = memoryStorage();
    const host = fakeHost();
    // Run 1: the create never ends (the app is quit while it downloads).
    host.nextCreates(() => new Promise<string>(() => {}));
    const a = new PrepareDriver(core, host.env(), storage);
    a.start("claude-code");
    a.progress({ phase: "pulling", detail: "image", bytesDone: 100, bytesTotal: 400 });
    a.send({ id: "m1", text: "keep me", agent: "claude-code" });
    await flush();
    expect(storage.saved?.messages.map((m) => m.id)).toEqual(["m1"]);
    expect(storage.saved?.image.state).toBe("running");
    // Run 2: a new page load with the same record.
    host.nextCreates(async () => "local:home-1");
    const b = new PrepareDriver(core, host.env(), storage);
    expect(b.state?.messages[0]?.text).toBe("keep me");
    b.resume();
    await flush();
    await flush();
    expect(host.calls.create).toBe(2);
    expect(host.calls.post).toEqual(["m1"]);
    expect(b.view()?.phase).toBe("ready");
    // A third launch with everything ready posts nothing again.
    const c = new PrepareDriver(core, host.env(), storage);
    c.resume();
    await flush();
    expect(host.calls.post).toEqual(["m1"]);
    expect(host.calls.create).toBe(2);
  });

  it("a quit while a message was being posted checks the thread: posted is not posted again, missing is posted once", async () => {
    const core = await testCore();
    for (const alreadyThere of [true, false]) {
      const storage = memoryStorage();
      const host = fakeHost();
      // Run 1: the post hangs (the app is quit before it answers).
      host.hold = new Promise<void>(() => {});
      const a = new PrepareDriver(core, host.env(), storage);
      a.start("claude-code");
      await flush();
      a.send({ id: "m1", text: "once", agent: "claude-code" });
      await flush();
      expect(storage.saved?.messages[0]?.state).toBe("delivering");
      // The post did reach the thread before the quit, or did not.
      if (alreadyThere) host.thread.push("once");
      host.hold = null;
      const b = new PrepareDriver(core, host.env(), storage);
      b.resume();
      await flush();
      await flush();
      expect(host.calls.verify).toBe(1);
      expect(host.thread.filter((t) => t === "once")).toHaveLength(1);
      expect(b.state?.messages[0]?.state).toBe("delivered");
      expect(host.calls.post).toEqual(alreadyThere ? [] : ["m1"]);
    }
  });

  it("a failed post goes back and is posted again later, still once", async () => {
    const core = await testCore();
    const host = fakeHost();
    const d = new PrepareDriver(core, host.env(), memoryStorage());
    d.start("claude-code");
    await flush();
    await flush();
    expect(d.view()?.phase).toBe("ready");
    host.postFails = true;
    d.send({ id: "m1", text: "later", agent: "claude-code" });
    await flush();
    expect(d.state?.messages[0]?.state).toBe("queued");
    host.postFails = false;
    await vi.advanceTimersByTimeAsync(2_000);
    await flush();
    expect(host.calls.post).toEqual(["m1"]);
    expect(d.state?.messages[0]?.state).toBe("delivered");
  });

  it("a failure that will not clear offers Try again, and Try again runs it", async () => {
    const core = await testCore();
    const host = fakeHost();
    host.nextCreates(
      async () => Promise.reject(new Error("no space left on device")),
      async () => Promise.reject(new Error("no space left on device")),
      async () => "local:home-1",
    );
    const d = new PrepareDriver(core, host.env(), memoryStorage());
    d.start("claude-code");
    await flush();
    await vi.advanceTimersByTimeAsync(2_000);
    await flush();
    expect(d.view()?.phase).toBe("failed");
    expect(d.view()?.action?.label).toBe("Try again");
    d.retry();
    await flush();
    await flush();
    expect(d.view()?.phase).toBe("ready");
  });

  it("a home that was removed is made again", async () => {
    const core = await testCore();
    const host = fakeHost();
    const d = new PrepareDriver(core, host.env(), memoryStorage());
    d.start("claude-code");
    await flush();
    await flush();
    expect(d.view()?.phase).toBe("ready");
    host.exists = false;
    host.nextCreates(async () => "local:home-2");
    d.checkHome();
    await flush();
    await flush();
    expect(host.calls.create).toBe(2);
    expect(d.state?.homeSpaceId).toBe("local:home-2");
    expect(d.view()?.phase).toBe("ready");
  });

  it("a create the host kept running while the app was quit is followed, not started a second time", async () => {
    const core = await testCore();
    const host = fakeHost();
    host.home = { id: "local:home-claude-code", running: false, creating: true };
    const d = new PrepareDriver(core, host.env(), memoryStorage());
    d.start("claude-code");
    await flush();
    await flush();
    expect(host.follows).toBe(1);
    expect(host.calls.create).toBe(0);
    expect(d.state?.homeSpaceId).toBe("local:home-claude-code");
    expect(d.view()?.phase).toBe("ready");
  });

  it("a followed create that fails is a failure with the host's words", async () => {
    const core = await testCore();
    const host = fakeHost();
    host.home = { id: "local:home-claude-code", running: false, creating: true };
    host.followResult = new Error("no space left on device");
    const d = new PrepareDriver(core, host.env(), memoryStorage());
    d.start("claude-code");
    await flush();
    expect(d.state?.trouble?.message).toBe("no space left on device");
    expect(host.calls.create).toBe(0);
  });

  it("an existing running home is used, not made again", async () => {
    const core = await testCore();
    const host = fakeHost();
    host.home = { id: "local:home-old", running: true };
    const d = new PrepareDriver(core, host.env(), memoryStorage());
    d.start("claude-code");
    await flush();
    await flush();
    expect(host.calls.create).toBe(0);
    expect(d.state?.homeSpaceId).toBe("local:home-old");
  });
});

describe("what counts as the connection's failure", () => {
  it.each([
    "connection reset by peer",
    "download the built-in Linux runtime: operation timed out",
    "dns error: failed to lookup address information",
    "the registry answered 503",
    "error sending request for url (https://ghcr.io/)",
  ])("%s", (m) => {
    expect(isNetworkFailure(m)).toBe(true);
  });
  it.each(["no space left on device", "the built-in Linux runtime cannot run from this path", "a disk image did not match its checksum"])("%s is not", (m) => {
    expect(isNetworkFailure(m)).toBe(false);
  });
});
