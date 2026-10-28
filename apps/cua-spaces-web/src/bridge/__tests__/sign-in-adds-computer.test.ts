// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import type { DataAdapter } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import { BridgeStore, type SessionData } from "../store";
import { testCore, tick, until, wasmBuilt } from "./testCore";

/** The demo host with this computer not set up yet (or set up), recording host setups. */
function host({ signedIn, configured }: { signedIn: boolean; configured: boolean }) {
  const demo = createDemoAdapter({ latencyMs: 0, stepMs: 0, signedIn, onboarded: true });
  const setups: Record<string, unknown>[] = [];
  let isSetUp = configured;
  const adapter = Object.assign(Object.create(demo) as DataAdapter, {
    call: (async (op: string, args: Record<string, unknown>) => {
      const out = (await demo.call(op as never, args as never)) as Record<string, unknown>;
      if (op === "host.status") return { ...out, configured: isSetUp };
      if (op === "host.setUp") {
        setups.push(args);
        isSetUp = true;
      }
      return out;
    }) as DataAdapter["call"],
  });
  return { adapter, setups };
}

describe.skipIf(!wasmBuilt)("signing in adds this computer", () => {
  it("a sign-in on a computer that isn't set up sets it up to provide Spaces, its screen private", async () => {
    const { adapter, setups } = host({ signedIn: false, configured: false });
    const store = new BridgeStore(adapter, await testCore());
    store.ensure("session");
    const session = () => store.get<SessionData>("session").data;
    await until(() => expect(session()?.signedIn).toBe(false));
    expect(setups).toHaveLength(0);

    await store.signIn();
    await until(() => expect(session()?.signedIn).toBe(true));
    await until(() => expect(setups).toHaveLength(1));
    const request = (setups[0] as { request?: Record<string, unknown> }).request ?? setups[0]!;
    // Provides Spaces, and doesn't share its desktop: that is a choice on its page.
    expect(request.profile).toBe("spare");
    store.dispose();
  });

  it("a computer already set up keeps its settings (Provide Spaces off stays off)", async () => {
    const { adapter, setups } = host({ signedIn: false, configured: true });
    const store = new BridgeStore(adapter, await testCore());
    store.ensure("session");
    const session = () => store.get<SessionData>("session").data;
    await until(() => expect(session()?.signedIn).toBe(false));
    await store.signIn();
    await until(() => expect(session()?.signedIn).toBe(true));
    await tick(50);
    expect(setups).toHaveLength(0);
    store.dispose();
  });

  it("opening the app already signed in sets nothing up (only a sign-in does)", async () => {
    const { adapter, setups } = host({ signedIn: true, configured: false });
    const store = new BridgeStore(adapter, await testCore());
    store.ensure("session");
    await until(() => expect(store.get<SessionData>("session").data?.signedIn).toBe(true));
    await tick(50);
    expect(setups).toHaveLength(0);
    store.dispose();
  });

  it("two sign-in signals set it up once", async () => {
    const { adapter, setups } = host({ signedIn: true, configured: false });
    const store = new BridgeStore(adapter, await testCore());
    await Promise.all([store.addThisComputer("ada@example.com"), store.addThisComputer("ada@example.com")]);
    expect(setups).toHaveLength(1);
    expect(await store.addThisComputer("ada@example.com")).toBe(true);
    expect(setups).toHaveLength(1);
    store.dispose();
  });
});
