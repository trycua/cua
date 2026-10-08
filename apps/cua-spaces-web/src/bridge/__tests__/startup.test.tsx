// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { BridgeProvider, useStartup, type StartupHook } from "..";
import { unavailableCore } from "../core";
import { createDemoAdapter, demoOptionsFromSearch } from "../adapters/demo";
import { DEMO_KEYCHAIN_WAIT_MS, demoStartup } from "../adapters/demo/startup";
import { createTauriAdapter } from "../adapters/tauri";
import { createWebkitAdapter } from "../adapters/webkit";
import type { HostWindow } from "../detect";
import { READY_STARTUP, startupFromHost, type StartupState } from "../ops/startup";
import type { HostEvent } from "../protocol";
import { WEBKIT_EVENT, type WebkitRequest } from "../webkit-protocol";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

type Answer = { ok: true; result: unknown } | { ok: false; error: { code: string; message: string } };

/** A SwiftUI host that answers with `answer`, and can push events. */
function webkitHost(answer: (req: WebkitRequest) => Answer) {
  const sent: WebkitRequest[] = [];
  const target = new EventTarget();
  const win = {
    webkit: {
      messageHandlers: {
        cua: {
          postMessage: (m: unknown) => {
            const req = m as WebkitRequest;
            sent.push(req);
            return Promise.resolve({ id: req.id, ...answer(req) });
          },
        },
      },
    },
    addEventListener: target.addEventListener.bind(target),
    removeEventListener: target.removeEventListener.bind(target),
  } as unknown as HostWindow;
  const push = (event: string, payload: unknown) => target.dispatchEvent(new CustomEvent(WEBKIT_EVENT, { detail: { event, payload } }));
  return { win, sent, push };
}

const KEYCHAIN: StartupState = demoStartup("needsKeychain");

describe("startup: SwiftUI host", () => {
  it("reads the host's state and sends its actions", async () => {
    const { win, sent } = webkitHost((req) => ({ ok: true, result: req.method === "startup.act" ? demoStartup("waitingForKeychain") : KEYCHAIN }));
    const a = createWebkitAdapter(win);
    expect(await a.call("startup.get", {})).toEqual(KEYCHAIN);
    expect((await a.call("startup.act", { action: "allowAccess" })).phase).toBe("waitingForKeychain");
    expect(sent.map((r) => [r.method, r.args])).toEqual([
      ["startup.get", {}],
      ["startup.act", { action: "allowAccess" }],
    ]);
  });

  it("reads an older build (unimplemented), a failure or a malformed answer as ready", async () => {
    const unimplemented = webkitHost(() => ({ ok: false, error: { code: "unimplemented", message: "startup.get is not available in this host" } }));
    expect(await createWebkitAdapter(unimplemented.win).call("startup.get", {})).toEqual(READY_STARTUP);
    expect(await createWebkitAdapter(unimplemented.win).call("startup.act", { action: "tryAgain" })).toEqual(READY_STARTUP);
    const failed = webkitHost(() => ({ ok: false, error: { code: "failed", message: "boom" } }));
    expect(await createWebkitAdapter(failed.win).call("startup.get", {})).toEqual(READY_STARTUP);
    const odd = webkitHost(() => ({ ok: true, result: { phase: "somethingNew", title: "?" } }));
    expect(await createWebkitAdapter(odd.win).call("startup.get", {})).toEqual(READY_STARTUP);
    const nothing = webkitHost(() => ({ ok: true, result: null }));
    expect(await createWebkitAdapter(nothing.win).call("startup.get", {})).toEqual(READY_STARTUP);
  });

  it("drops actions it doesn't know and passes startup.changed on with its state", async () => {
    const { win, push } = webkitHost(() => ({ ok: true, result: { ...KEYCHAIN, actions: ["allowAccess", "reboot", "signInAgain"] } }));
    const a = createWebkitAdapter(win);
    expect((await a.call("startup.get", {})).actions).toEqual(["allowAccess", "signInAgain"]);
    const events: HostEvent[] = [];
    a.subscribe((e) => events.push(e));
    push("startup.changed", demoStartup("keychainDenied"));
    push("startup.changed", { phase: "ready" });
    expect(events).toEqual([
      { type: "startup.changed", state: demoStartup("keychainDenied") },
      { type: "startup.changed", state: READY_STARTUP },
    ]);
  });
});

describe("startup: Tauri", () => {
  it("has no startup gate: ready, with no command", async () => {
    const seen: string[] = [];
    const win = { __TAURI_INTERNALS__: { invoke: async (c: string) => (seen.push(c), null) }, __CUA_UI_STORAGE__: {} } as unknown as HostWindow;
    const a = createTauriAdapter(win);
    expect(await a.call("startup.get", {})).toEqual(READY_STARTUP);
    expect(await a.call("startup.act", { action: "allowAccess" })).toEqual(READY_STARTUP);
    expect(seen).toEqual([]);
  });
});

describe("startup: demo", () => {
  it("is ready by default", async () => {
    const a = createDemoAdapter({ latencyMs: 0 });
    expect(await a.call("startup.get", {})).toEqual(READY_STARTUP);
    expect(await a.call("startup.act", { action: "allowAccess" })).toEqual(READY_STARTUP);
    a.dispose?.();
  });

  it("?demo=keychain walks the Keychain prompt, combined with other flags", async () => {
    vi.useFakeTimers();
    const options = demoOptionsFromSearch("?demo=fresh,keychain");
    expect(options).toMatchObject({ keychain: true, signedIn: false });
    const a = createDemoAdapter({ ...options, latencyMs: 0 });
    const events: HostEvent[] = [];
    a.subscribe((e) => events.push(e));
    const call = async <T,>(p: Promise<T>) => {
      await vi.advanceTimersByTimeAsync(0);
      return p;
    };

    const first = await call(a.call("startup.get", {}));
    expect(first).toMatchObject({ phase: "needsKeychain", title: "Allow Keychain access", actions: ["allowAccess", "signInAgain"] });

    // Allow access: the prompt is up, then answered.
    expect(await call(a.call("startup.act", { action: "allowAccess" }))).toMatchObject({ phase: "waitingForKeychain", title: "Waiting for Keychain access…", actions: [] });
    await vi.advanceTimersByTimeAsync(DEMO_KEYCHAIN_WAIT_MS);
    expect(await call(a.call("startup.get", {}))).toEqual(READY_STARTUP);
    expect(events.map((e) => (e.type === "startup.changed" ? e.state.phase : e.type))).toEqual(["waitingForKeychain", "ready"]);

    // Try again waits again; Sign in again is ready at once (and the old wait does nothing).
    a.state.startup = demoStartup("keychainDenied");
    expect((await call(a.call("startup.act", { action: "tryAgain" }))).phase).toBe("waitingForKeychain");
    expect(await call(a.call("startup.act", { action: "signInAgain" }))).toEqual(READY_STARTUP);
    events.length = 0;
    await vi.advanceTimersByTimeAsync(DEMO_KEYCHAIN_WAIT_MS);
    expect(events).toEqual([]);
    a.dispose?.();
  });

  it("uses the native words for every phase", () => {
    expect(demoStartup("waitingForKeychain", true)).toMatchObject({ title: "Still waiting for Keychain access", actions: ["tryAgain", "signInAgain"] });
    expect(demoStartup("keychainDenied")).toMatchObject({ title: "Keychain access was not allowed", actions: ["tryAgain", "signInAgain"] });
    expect(demoStartup("startFailed")).toMatchObject({ phase: "startFailed", actions: ["tryAgain"] });
    expect(startupFromHost(demoStartup("startFailed")).phase).toBe("startFailed");
    expect(demoStartup("starting")).toEqual({ phase: "starting", slow: false, title: "Starting Cua…", body: "", actions: [] });
    expect(demoStartup("starting", true)).toMatchObject({ title: "Still starting Cua…", body: "This can take a minute after an update." });
  });
});

describe("useStartup", () => {
  const probe = {} as { hook: StartupHook };
  function Probe() {
    probe.hook = useStartup();
    return <span data-testid="phase">{probe.hook.isLoading ? "loading" : probe.hook.data.phase}</span>;
  }

  it("reads the state, follows startup.changed and sends actions", async () => {
    const adapter = createDemoAdapter({ latencyMs: 1, keychain: true });
    render(
      <BridgeProvider adapter={adapter} core={unavailableCore("test")}>
        <Probe />
      </BridgeProvider>,
    );
    expect(screen.getByTestId("phase").textContent).toBe("loading");
    expect(probe.hook.data).toEqual(READY_STARTUP);
    await waitFor(() => expect(screen.getByTestId("phase").textContent).toBe("needsKeychain"));
    await act(() => probe.hook.act("signInAgain"));
    expect(screen.getByTestId("phase").textContent).toBe("ready");
    adapter.dispose?.();
  });

  it("reads a host that fails as ready", async () => {
    const adapter = createDemoAdapter({ latencyMs: 1 });
    adapter.call = (() => Promise.reject(new Error("no host"))) as typeof adapter.call;
    render(
      <BridgeProvider adapter={adapter} core={unavailableCore("test")}>
        <Probe />
      </BridgeProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("phase").textContent).toBe("ready"));
    expect(probe.hook.error?.message).toBe("no host");
  });
});
