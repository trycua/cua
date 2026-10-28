// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import type { DataAdapter } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import { BridgeStore, type SessionData } from "../store";
import { unavailableCore } from "../core";
import { testCore, until, wasmBuilt } from "./testCore";

/** The demo host, with a browser sign-in that never finishes. */
function stuckSignIn(): DataAdapter & { state: ReturnType<typeof createDemoAdapter>["state"] } {
  const demo = createDemoAdapter({ latencyMs: 0, signedIn: false, onboarded: false });
  return Object.assign(Object.create(demo), {
    call: ((method: string, args: unknown) =>
      method === "session.signIn"
        ? Promise.resolve({ method: "device", userCode: "ABCD-EFGH", verificationUri: "https://cua.ai/device" })
        : demo.call(method as never, args as never)) as DataAdapter["call"],
    state: demo.state,
  });
}

describe.skipIf(!wasmBuilt)("a sign-in that never finishes", () => {
  it("shows the code and the page to reopen, can be cancelled, and times out", async () => {
    const adapter = stuckSignIn();
    const store = new BridgeStore(adapter, await testCore(), { signInTimeoutMs: 40 });
    store.ensure("session");
    store.ensure("settings");
    const session = () => store.get<SessionData>("session").data;
    await until(() => expect(session()).toBeTruthy());

    await store.signIn();
    expect(session()?.signIn).toEqual({ kind: "waiting", userCode: "ABCD-EFGH" });
    expect(session()?.signInUrl).toBe("https://cua.ai/device");
    store.cancelSignIn();
    expect(session()?.signIn).toEqual({ kind: "idle" });
    expect(session()?.signInUrl).toBeNull();

    await store.signIn();
    await until(() =>
      expect(session()?.signIn).toEqual({
        kind: "failed",
        reason: "notConfirmed",
        message: "Sign-in didn't finish. The browser didn't confirm in time.",
        detail: null,
      }),
    );
    await store.telemetry.flush();
    const kinds = adapter.state.telemetry.flatMap((s) => (s.type === "sign-in-failed" ? [s.errorKind] : []));
    expect(kinds).toEqual(["cancelled", "timeout"]);
    store.dispose();
  });
});

/** A native host (the Electron app): it runs the sign-in, says when it waits
 * (a new page per attempt), and only `session.changed` says it finished. */
function nativeHost() {
  const demo = createDemoAdapter({ latencyMs: 0, signedIn: false, onboarded: true });
  const listeners = new Set<(e: { type: string }) => void>();
  const host = {
    attempts: 0,
    identity: null as string | null,
    failure: null as { reason: string; message: string; detail: string | null } | null,
    /** How long `session.get` takes to answer (ms). */
    getDelay: 0,
  };
  const emit = () => listeners.forEach((l) => l({ type: "session.changed" }));
  const adapter = Object.assign(Object.create(demo) as DataAdapter, {
    subscribe: (l: (e: { type: string }) => void) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    call: (async (op: string, args: unknown) => {
      if (op === "session.signIn") {
        host.attempts += 1;
        setTimeout(emit, 1);
        return { method: "browser", verificationUri: "" };
      }
      const out = (await demo.call(op as never, args as never)) as Record<string, unknown>;
      if (op !== "session.get") return out;
      // What the host says now; it answers after `getDelay`.
      const failure = host.failure;
      if (host.getDelay) await new Promise((r) => setTimeout(r, host.getDelay));
      const account = out.account as Record<string, unknown>;
      return host.identity
        ? { ...out, account: { ...account, configured: true, authMode: "user", identity: host.identity }, hostSignIn: null }
        : failure
          ? { ...out, hostSignIn: null, hostSignInFailure: failure }
          : { ...out, hostSignIn: { userCode: null, url: `http://127.0.0.1:${50000 + host.attempts}/callback` } };
    }) as DataAdapter["call"],
  });
  return { adapter, host, emit };
}

describe("a sign-in the host finished on a retried attempt (CUA-1728)", () => {
  it("stops waiting once the host is signed in", async () => {
    const { adapter, host, emit } = nativeHost();
    const store = new BridgeStore(adapter, unavailableCore("test: no core"));
    store.ensure("session");
    const session = () => store.get<SessionData>("session").data;
    await until(() => expect(session()).toBeTruthy());

    await store.signIn();
    await until(() => expect(session()?.signInUrl).toBe("http://127.0.0.1:50001/callback"));
    // The first attempt's browser came too late: try again, a new port.
    store.cancelSignIn();
    await store.signIn();
    await until(() => expect(session()?.signInUrl).toBe("http://127.0.0.1:50002/callback"));
    expect(session()?.signIn.kind).toBe("waiting");

    host.identity = "ada@example.com";
    emit();
    await until(() => expect(session()?.signIn).toEqual({ kind: "idle" }));
    expect(session()?.signedIn).toBe(true);
    expect(session()?.signInUrl).toBeNull();
    store.dispose();
  });
});

describe("a sign-in that fails in the host (Ellie: it silently failed)", () => {
  it("shows the host's reason at once instead of waiting on", async () => {
    const { adapter, host, emit } = nativeHost();
    const store = new BridgeStore(adapter, unavailableCore("test: no core"));
    store.ensure("session");
    const session = () => store.get<SessionData>("session").data;
    await until(() => expect(session()).toBeTruthy());

    await store.signIn();
    await until(() => expect(session()?.signIn.kind).toBe("waiting"));
    // The network dropped while the browser was open: the host gave up.
    host.failure = { reason: "offline", message: "Sign-in didn't finish. Can't reach Cua. Check your connection.", detail: "dns error" };
    emit();
    await until(() =>
      expect(session()?.signIn).toEqual({
        kind: "failed",
        reason: "offline",
        message: "Sign-in didn't finish. Can't reach Cua. Check your connection.",
        detail: "dns error",
      }),
    );
    expect(session()?.signInUrl).toBeNull();
    await store.telemetry.flush();
    store.dispose();
  });

  it("never reads an earlier attempt's failure as the new one's", async () => {
    const { adapter, host, emit } = nativeHost();
    const store = new BridgeStore(adapter, unavailableCore("test: no core"));
    store.ensure("session");
    const session = () => store.get<SessionData>("session").data;
    await until(() => expect(session()).toBeTruthy());
    // The last attempt failed on the host; a session read starts (slow)...
    host.failure = { reason: "notConfirmed", message: "Sign-in didn't finish. The browser didn't confirm in time.", detail: null };
    host.getDelay = 30;
    emit();
    await new Promise((r) => setTimeout(r, 5));
    // ...and Try again is pressed before it answers.
    host.failure = null;
    await store.signIn();
    expect(session()?.signIn.kind).toBe("waiting");
    // The slow read lands with the old failure: not this attempt's.
    await new Promise((r) => setTimeout(r, 60));
    expect(session()?.signIn.kind).toBe("waiting");
    store.dispose();
  });
});
