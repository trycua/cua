// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createDemoAdapter } from "../adapters/demo";
import { demoRelayUsage } from "../adapters/demo/relay-usage";
import { createWebkitAdapter } from "../adapters/webkit";
import { BridgeProvider, isUnsupported, relayUsageLine, useRelayUsage, type DataAdapter, type RelayUsage } from "..";
import type { HostWindow } from "../detect";
import { RELAY_USAGE_POLL_MS } from "../relay-usage";
import { noCore } from "./testCore";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

function Probe({ seen }: { seen: { usage?: RelayUsage | null } }) {
  seen.usage = useRelayUsage();
  return null;
}

describe("relay.usage", () => {
  it("the demo host answers a free plan partway through the month, and null signed out", async () => {
    expect(await createDemoAdapter({ latencyMs: 0 }).call("relay.usage", {})).toEqual(demoRelayUsage());
    expect(await createDemoAdapter({ latencyMs: 0, signedIn: false }).call("relay.usage", {})).toBeNull();
  });

  it("the SwiftUI host does not answer it, and the page shows nothing", async () => {
    const postMessage = vi.fn();
    const win = { webkit: { messageHandlers: { cua: { postMessage } } }, addEventListener: () => {}, removeEventListener: () => {} } as unknown as HostWindow;
    const adapter = createWebkitAdapter(win);
    expect(isUnsupported(await adapter.call("relay.usage", {}).catch((e: unknown) => e))).toBe(true);
    expect(postMessage).not.toHaveBeenCalled();

    const seen: { usage?: RelayUsage | null } = {};
    render(
      <BridgeProvider adapter={adapter} core={noCore}>
        <Probe seen={seen} />
      </BridgeProvider>,
    );
    await act(() => new Promise((r) => setTimeout(r, 10)));
    expect(seen.usage).toBeNull();
  });

  it("reads once for every screen, again every 5 minutes, and drops it when signed out", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const demo = createDemoAdapter({ latencyMs: 0 });
    const call = vi.fn(demo.call);
    const adapter: DataAdapter = { ...demo, call };
    const seen: { usage?: RelayUsage | null } = {};
    render(
      <BridgeProvider adapter={adapter} core={noCore}>
        <Probe seen={seen} />
        <Probe seen={{}} />
      </BridgeProvider>,
    );
    await act(() => new Promise((r) => setTimeout(r, 10)));
    const reads = () => call.mock.calls.filter(([op]) => op === "relay.usage").length;
    expect(reads()).toBe(1);
    expect(seen.usage).toMatchObject({ plan: "free", liveSessions: 1 });
    expect(relayUsageLine(seen.usage!)).toBe("Live sessions 1 of 2 · Relay hours 6.5 of 20 this month");

    await act(() => vi.advanceTimersByTimeAsync(RELAY_USAGE_POLL_MS - 100));
    expect(reads()).toBe(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(200);
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(reads()).toBe(2);

    await act(async () => {
      await demo.call("session.signOut", {});
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(seen.usage).toBeNull();
  });

  it("an account change drops an answer still in flight and reads for the new account", async () => {
    const demo = createDemoAdapter({ latencyMs: 0 });
    const listeners = new Set<(e: { type: string }) => void>();
    let release: (v: RelayUsage | null) => void = () => {};
    let n = 0;
    const call = vi.fn((op: string, args: unknown) => {
      if (op !== "relay.usage") return demo.call(op as never, args as never);
      n += 1;
      // The first read hangs until released; later ones answer at once.
      if (n === 1) return new Promise<RelayUsage | null>((r) => (release = r));
      return Promise.resolve({ ...demoRelayUsage(), plan: "pro" });
    });
    const adapter = {
      ...demo,
      call,
      subscribe: (l: (e: { type: string }) => void) => {
        listeners.add(l);
        return () => listeners.delete(l);
      },
    } as unknown as DataAdapter;
    const seen: { usage?: RelayUsage | null } = {};
    render(
      <BridgeProvider adapter={adapter} core={noCore}>
        <Probe seen={seen} />
      </BridgeProvider>,
    );
    await act(() => new Promise((r) => setTimeout(r, 10)));
    await act(async () => {
      for (const l of listeners) l({ type: "session.changed" });
      release({ ...demoRelayUsage(), plan: "free" });
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(n).toBe(2);
    expect(seen.usage).toMatchObject({ plan: "pro" });
  });

  it("a relay that meters nothing reads as nothing to show", async () => {
    const demo = createDemoAdapter({ latencyMs: 0 });
    demo.state.relayUsage = { ...demoRelayUsage(), metered: false };
    const seen: { usage?: RelayUsage | null } = {};
    render(
      <BridgeProvider adapter={demo} core={noCore}>
        <Probe seen={seen} />
      </BridgeProvider>,
    );
    await act(() => new Promise((r) => setTimeout(r, 10)));
    expect(seen.usage).toBeNull();
  });
});
