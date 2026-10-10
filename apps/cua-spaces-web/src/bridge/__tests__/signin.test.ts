// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import type { DataAdapter } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import { BridgeStore, SIGN_IN_TIMED_OUT, type SessionData } from "../store";
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
    await until(() => expect(session()?.signIn).toEqual({ kind: "failed", message: SIGN_IN_TIMED_OUT }));
    await store.telemetry.flush();
    const kinds = adapter.state.telemetry.flatMap((s) => (s.type === "sign-in-failed" ? [s.errorKind] : []));
    expect(kinds).toEqual(["cancelled", "timeout"]);
    store.dispose();
  });
});
