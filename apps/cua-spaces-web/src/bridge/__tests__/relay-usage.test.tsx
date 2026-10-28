// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createDemoAdapter } from "../adapters/demo";
import { demoRelayUsage } from "../adapters/demo/relay-usage";
import { createWebkitAdapter } from "../adapters/webkit";
import { BridgeProvider, UnsupportedOperationError, useRelayUsage, type DataAdapter, type RelayUsage } from "..";
import { relayUsageView } from "../derive";
import type { HostWindow } from "../detect";
import { RELAY_USAGE_POLL_MS } from "../relay-usage";
import { noCore, testCore, wasmBuilt } from "./testCore";

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

  it("the SwiftUI host answers it with the same method", async () => {
    const postMessage = vi.fn(async (req: { id: string; method: string }) => ({ id: req.id, ok: true, result: demoRelayUsage() }));
    const win = { webkit: { messageHandlers: { cua: { postMessage } } }, addEventListener: () => {}, removeEventListener: () => {} } as unknown as HostWindow;
    expect(await createWebkitAdapter(win).call("relay.usage", {})).toEqual(demoRelayUsage());
    expect(postMessage).toHaveBeenCalledWith(expect.objectContaining({ method: "relay.usage" }));
  });

  it("a host without it reads as nothing to show", async () => {
    const demo = createDemoAdapter({ latencyMs: 0 });
    const adapter: DataAdapter = {
      ...demo,
      call: (op, args) => (op === "relay.usage" ? Promise.reject(new UnsupportedOperationError("tauri", "relay.usage")) : demo.call(op, args)),
    };
    const seen: { usage?: RelayUsage | null } = {};
    render(
      <BridgeProvider adapter={adapter} core={noCore}>
        <Probe seen={seen} />
      </BridgeProvider>,
    );
    await act(() => new Promise((r) => setTimeout(r, 10)));
    expect(seen.usage).toBeNull();
  });

  it.skipIf(!wasmBuilt)("the row and the notice are the core's words", async () => {
    const core = await testCore();
    const free = relayUsageView(core, demoRelayUsage())!;
    expect(free).toMatchObject({ label: "Remote use", line: "Live sessions 1 of 2 · Remote hours 6.5 of 20 this month", plan: "Free plan", notice: null });
    expect(free.planCard).toMatchObject({ plan: "free", label: "Free plan", primary: { label: "Get Pro", url: "https://app.cua.ai/buy/pro?from=app_plan_card" }, secondary: { label: "Start a team", url: "https://app.cua.ai/buy/teams?from=app_plan_card" } });
    const past = relayUsageView(core, { ...demoRelayUsage(), hoursUsed: 20.24, softLimitReached: true, reasons: ["hours"], team: { id: "acme", seats: 3 } })!;
    expect(past.line).toBe("Live sessions 1 of 2 · Remote hours 20.2 of 20 this month · Team pool, 3 seats");
    expect(past.notice).toEqual({
      text: "You're using 20.2 of 20 remote hours on the free plan. Your Spaces keep working. Pro gives you 5 live sessions and 200 remote hours.",
      upgrade: "Upgrade",
      upgradeUrl: "https://app.cua.ai/buy/pro?from=app_meter",
      dismiss: "Dismiss",
      dismissKey: "2026-10:hours",
      level: "limit",
      // For usage data (CUA-1881): which cap, which plan.
      kind: "hours",
      plan: "free",
    });
    expect(relayUsageView(noCore, demoRelayUsage())).toBeNull();
  });

  it("reads once for every screen, again every minute, and drops it when signed out", async () => {
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
