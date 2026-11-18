// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { DataAdapter } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import { AgentKeysStore, type FocusTarget } from "../agent-keys";
import type { HostEvent, OpArgs, OpName } from "../protocol";

afterEach(cleanup);

/** The demo host, with every call logged and a way to send it events. */
function loggedDemo() {
  const demo = createDemoAdapter({ latencyMs: 0, stepMs: 2 });
  const calls: [string, unknown][] = [];
  const listeners = new Set<(e: HostEvent) => void>();
  const adapter: DataAdapter = {
    mode: demo.mode,
    call: (op, args) => {
      calls.push([op, args]);
      return demo.call(op, args);
    },
    subscribe: (l) => {
      listeners.add(l);
      const off = demo.subscribe(l);
      return () => {
        listeners.delete(l);
        off();
      };
    },
    dispose: () => demo.dispose?.(),
  };
  return {
    adapter,
    demo,
    calls,
    emit: (e: HostEvent) => listeners.forEach((l) => l(e)),
    lists: () => calls.filter(([op]) => op === "agentKeys.list").length,
  };
}

/** A window and a document that only count their listeners and fire them. */
function fakeTarget() {
  const on: Record<string, Set<() => void>> = {};
  const add = (t: string, l: () => void) => (on[t] ??= new Set()).add(l);
  const remove = (t: string, l: () => void) => on[t]?.delete(l);
  const doc = { visibilityState: "visible", addEventListener: add, removeEventListener: remove };
  const target: FocusTarget = { addEventListener: add, removeEventListener: remove, document: doc };
  return { target, doc, fire: (t: string) => [...(on[t] ?? [])].forEach((l) => l()), count: () => Object.values(on).reduce((n, s) => n + s.size, 0) };
}

describe("Settings, Connections keeps its list fresh", () => {
  it("reads when watching starts, on focus, on a visible document and on agents.changed", async () => {
    const h = loggedDemo();
    const store = new AgentKeysStore(h.adapter);
    const w = fakeTarget();
    const stop = store.watch(w.target);
    await store.refresh();
    expect(h.lists()).toBe(1);

    // A key removed outside the app (the CLI) shows once the window is back.
    await h.demo.call("agentKeys.remove", { env: "OPENAI_API_KEY" });
    expect(store.get().report?.keys.map((k) => k.env)).toEqual(["OPENAI_API_KEY"]);
    w.fire("focus");
    await waitFor(() => expect(store.get().report?.keys).toEqual([]));
    expect(h.lists()).toBe(2);

    await h.demo.call("agentKeys.set", { provider: "anthropic", value: "sk-ant-test-0000" });
    w.doc.visibilityState = "hidden";
    w.fire("visibilitychange");
    await store.refresh();
    expect(h.lists()).toBe(2 + 1); // hidden: no read of its own (this one is the explicit refresh)
    expect(store.get().report?.keys.map((k) => k.env)).toEqual(["ANTHROPIC_API_KEY"]);

    await h.demo.call("agentKeys.remove", { env: "ANTHROPIC_API_KEY" });
    w.doc.visibilityState = "visible";
    w.fire("visibilitychange");
    await waitFor(() => expect(store.get().report?.keys).toEqual([]));

    await h.demo.call("agentKeys.set", { provider: "openai", value: "sk-test-1111" });
    h.emit({ type: "agents.changed" });
    await waitFor(() => expect(store.get().report?.keys.map((k) => k.env)).toEqual(["OPENAI_API_KEY"]));
    h.emit({ type: "spaces.changed" });
    const before = h.lists();
    await Promise.resolve();
    expect(h.lists()).toBe(before);

    // Stopped: nothing listens any more.
    stop();
    expect(w.count()).toBe(0);
    const after = h.lists();
    w.fire("focus");
    h.emit({ type: "agents.changed" });
    expect(h.lists()).toBe(after);
    h.adapter.dispose?.();
  });

  it("shares one read between callers", async () => {
    const h = loggedDemo();
    const store = new AgentKeysStore(h.adapter);
    await Promise.all([store.refresh(), store.refresh(), store.refresh()]);
    expect(h.lists()).toBe(1);
    await store.refresh();
    expect(h.lists()).toBe(2);
    h.adapter.dispose?.();
  });

  it("never lets a read from before a save replace the save's answer", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 2 });
    let releaseList: (() => void) | undefined;
    const slow: DataAdapter = {
      mode: demo.mode,
      subscribe: (l) => demo.subscribe(l),
      call: async <K extends OpName>(op: K, args: OpArgs<K>) => {
        const answer = await demo.call(op, args);
        // The list read is answered late, after the save below went through.
        if (op === "agentKeys.list") await new Promise<void>((r) => (releaseList = r));
        return answer;
      },
    };
    const store = new AgentKeysStore(slow);
    const reading = store.refresh();
    await waitFor(() => expect(releaseList).toBeDefined());
    await store.save("anthropic", "sk-ant-test-0000", null);
    releaseList?.();
    await reading;
    expect(store.get().report?.keys.map((k) => k.env)).toEqual(["ANTHROPIC_API_KEY", "OPENAI_API_KEY"]);
    demo.dispose?.();
  });
});
