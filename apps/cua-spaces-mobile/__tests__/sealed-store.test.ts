// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Cache } from "../src/store/cache";
import { createSealedStore } from "../src/store/sealed";
import { fakeSealer, memoryKv, testStore } from "./helpers/store";

describe("the sealed store", () => {
  it("keeps values sealed under their slot, and a value moved to another slot does not open", async () => {
    const kv = memoryKv();
    const s = testStore(kv);
    await s.set("outbox", '{"messages":[]}');
    expect(await s.get("outbox")).toBe('{"messages":[]}');
    const [key] = [...kv.data.keys()];
    expect(key).toBe("cua.sealed.v1:test:outbox");
    // Move the sealed text to another slot: it is gone, not opened.
    kv.data.set("cua.sealed.v1:test:thread:x", kv.data.get(key!)!);
    expect(await s.get("thread:x")).toBeNull();
    expect(kv.data.has("cua.sealed.v1:test:thread:x")).toBe(false);
  });

  it("is honestly unavailable without a sealer: nothing is kept, and nothing is read", async () => {
    const kv = memoryKv();
    const s = createSealedStore({ sealer: null, storage: kv });
    expect(s.available).toBe(false);
    await s.set("outbox", "secret");
    expect(kv.data.size).toBe(0);
    expect(await s.get("outbox")).toBeNull();
  });

  it("clears everything it wrote and drops the key on sign-out", async () => {
    const kv = memoryKv();
    kv.data.set("other-app-key", "stay");
    const s = testStore(kv);
    await s.set("a", "1");
    await s.set("b", "2");
    const before = fakeSealer.resets;
    await s.clear();
    expect([...kv.data.keys()]).toEqual(["other-app-key"]);
    expect(fakeSealer.resets).toBe(before + 1);
  });
});

describe("cached threads and the Spaces list survive a restart", () => {
  const thread = { entries: [{ seq: 1, text: "hello" }] as never, members: [], me: "acct:you", role: "owner", readSeq: 1 };

  it("writes a thread soon after it changes, reads it back in the next launch, and keeps the newest state", async () => {
    jest.useFakeTimers();
    try {
      const kv = memoryKv();
      let t = 5_000;
      const a = new Cache(testStore(kv), () => t, 100);
      a.saveThread("relay:a", thread);
      t = 6_000;
      a.saveThread("relay:a", { ...thread, readSeq: 9 });
      await jest.advanceTimersByTimeAsync(200);
      const b = new Cache(testStore(kv));
      const got = await b.loadThread("relay:a");
      expect(got).toMatchObject({ readSeq: 9, savedMs: 6_000, me: "acct:you" });
      expect(await b.loadThread("relay:never")).toBeNull();
    } finally {
      jest.useRealTimers();
    }
  });

  it("flushes a pending write when the app goes to the background", async () => {
    const kv = memoryKv();
    const a = new Cache(testStore(kv), () => 1, 60_000);
    a.saveThread("relay:a", thread);
    await a.flush();
    expect(await new Cache(testStore(kv)).loadThread("relay:a")).not.toBeNull();
  });

  it("keeps the last Spaces list with when it was saved, and forgets it all on sign-out", async () => {
    const kv = memoryKv();
    const a = new Cache(testStore(kv), () => 42);
    await a.saveSpaces({ spaces: [{ id: "relay:a", name: "checkout-api" }], machines: [], incomplete: false, reaching: 0, agents: [] } as never);
    const b = new Cache(testStore(kv));
    expect(await b.loadSpaces()).toMatchObject({ savedMs: 42, list: { spaces: [{ name: "checkout-api" }] } });
    await b.clear();
    expect(await new Cache(testStore(kv)).loadSpaces()).toBeNull();
  });

  it("keeps only the most recent few dozen threads", async () => {
    const kv = memoryKv();
    let t = 0;
    const c = new Cache(testStore(kv), () => ++t, 0);
    for (let i = 0; i < 43; i++) {
      c.saveThread(`relay:t${i}`, thread);
      await c.flush();
    }
    expect(await c.loadThread("relay:t0")).toBeNull();
    expect(await c.loadThread("relay:t42")).not.toBeNull();
  });
});
