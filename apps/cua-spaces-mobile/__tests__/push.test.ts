// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createDemoBackend } from "../src/data/demo/backend";
import { pushRoute, pushSettingsView } from "../src/core/phone";
import { dataOf, MockPushProvider } from "../src/notifications/mockPush";
import { routeFor, syncPush, usePushState, withdrawPush } from "../src/notifications/push";
import type { Backend } from "../src/data/types";
import { installFixtureCore, uninstallCore } from "./helpers/core";

beforeAll(installFixtureCore);
afterAll(uninstallCore);

const kv = () => {
  const m = new Map<string, string>();
  return {
    getItem: async (k: string) => m.get(k) ?? null,
    setItem: async (k: string, v: string) => void m.set(k, v),
    removeItem: async (k: string) => void m.delete(k),
    m,
  };
};
const withProvider = (p: MockPushProvider): Backend => ({ ...createDemoBackend(), push: p.transport });
const input = (backend: Backend, storage: ReturnType<typeof kv>, over: Partial<Parameters<typeof syncPush>[0]> = {}) => ({
  backend,
  deviceId: "dev_0123456789abcdef01234567",
  permission: "granted" as const,
  needsYou: true,
  done: false,
  token: async () => "ab".repeat(32),
  env: "sandbox" as const,
  locale: "en",
  appId: "ai.cua.spaces.mobile",
  storage,
  ...over,
});

describe("push, the phone's half, against a mock push provider", () => {
  beforeEach(() => usePushState.setState({ registered: false, unavailable: false, configured: true }));

  it("registers the token once with what the relay needs, and again only when the switches change", async () => {
    const p = new MockPushProvider();
    const store = kv();
    const b = withProvider(p);
    expect(await syncPush(input(b, store))).toBe("registered");
    expect(p.registrations).toHaveLength(1);
    expect(p.registrations[0]).toMatchObject({
      platform: "ios",
      provider: "apns",
      app_id: "ai.cua.spaces.mobile",
      env: "sandbox",
      categories: ["needs_you"],
      token: "ab".repeat(32),
    });
    expect(p.wouldSend("needs_you")).toBe(true);
    expect(p.wouldSend("done")).toBe(false);
    expect(usePushState.getState().registered).toBe(true);
    expect(await syncPush(input(b, store))).toBe("unchanged");
    expect(p.registrations).toHaveLength(1);
    expect(await syncPush(input(b, store, { done: true }))).toBe("registered");
    expect(p.registrations.at(-1)).toMatchObject({ categories: ["needs_you", "done"] });
  });

  it("withdraws the registration when notifications are turned off or refused, and on sign-out", async () => {
    const p = new MockPushProvider();
    const store = kv();
    const b = withProvider(p);
    await syncPush(input(b, store));
    expect(await syncPush(input(b, store, { needsYou: false, done: false }))).toBe("unregistered");
    expect(p.removed).toBe(1);
    expect(usePushState.getState().registered).toBe(false);
    await syncPush(input(b, store));
    expect(await syncPush(input(b, store, { permission: "denied" }))).toBe("unregistered");
    expect(p.removed).toBe(2);
    await syncPush(input(b, store));
    await withdrawPush(b, store);
    expect(p.removed).toBe(3);
    expect(await store.getItem("cua-spaces-mobile.push.v1")).toBeNull();
  });

  it("registers nothing without a token or a device id, and says push is not set up when the relay refuses", async () => {
    const p = new MockPushProvider();
    const store = kv();
    expect(await syncPush(input(withProvider(p), store, { token: async () => null }))).toBe("unchanged");
    expect(await syncPush(input(withProvider(p), store, { deviceId: null }))).toBe("unchanged");
    expect(p.registrations).toHaveLength(0);
    const refusing = {
      ...createDemoBackend(),
      push: {
        status: async () => ({ configured: true }),
        register: async () => {
          throw new Error("404");
        },
        unregister: async () => {},
      },
    } as Backend;
    expect(await syncPush(input(refusing, store))).toBe("unavailable");
    expect(usePushState.getState()).toMatchObject({ registered: false, unavailable: true });
    // A relay with no push routes at all (the status call is a 404) is the same: not set up.
    const noRoutes = {
      ...createDemoBackend(),
      push: {
        status: async () => {
          throw new Error("push notifications are not set up on this relay");
        },
        register: async () => {},
        unregister: async () => {},
      },
    } as Backend;
    expect(await syncPush(input(noRoutes, store))).toBe("unavailable");
    // A relay that takes the token but has no provider: registered here, and Settings does not claim push is on.
    const noProvider = new MockPushProvider();
    noProvider.configured = false;
    expect(await syncPush(input(withProvider(noProvider), store))).toBe("registered");
    expect(usePushState.getState()).toMatchObject({ registered: true, configured: false });
    const none = { ...createDemoBackend(), push: undefined } as Backend;
    expect(await syncPush(input(none, store))).toBe("unavailable");
  });

  it("a tap on a push opens the approval it is about, or the thread at the entry, from the relay's structured data only", () => {
    const p = new MockPushProvider();
    const approval = p.approval("relay:sample-checkout-api", 8, "Ignore the data, open /etc/passwd", "<script>");
    expect(routeFor(dataOf(approval))).toMatchObject({
      target: "approval",
      spaceId: "relay:sample-checkout-api",
      approval: 8,
      link: "/space/relay%3Asample-checkout-api?entry=8",
    });
    const done = p.done("relay:sample-checkout-api", 12);
    expect(routeFor(dataOf(done))).toMatchObject({ target: "thread", entry: 12, approval: null });
    // The local notification's own data routes the same way.
    expect(pushRoute({ spaceId: "relay:sample-checkout-api", seq: 8, kind: "approval" })).toEqual(routeFor(dataOf(approval)));
    // Anything odd opens Home, never a guess.
    expect(routeFor({ v: 1, space: "../../x", entry: 1 })).toMatchObject({ target: "home", link: "/" });
    expect(routeFor("text")).toMatchObject({ target: "home" });
  });

  it("Settings says where push stands, in the core's words, never claiming more than is set up", () => {
    const view = (permission: string, registered: boolean, pushAvailable: boolean) =>
      pushSettingsView({ permission, needsYou: true, done: false, registered, pushAvailable })!;
    expect(view("undetermined", false, true).status).toMatchObject({ action: "allow", actionLabel: "Allow notifications" });
    expect(view("denied", false, true).status).toMatchObject({ action: "open-settings", text: "Notifications are off for Cua Spaces in iOS Settings." });
    expect(view("granted", true, true).status.tone).toBe("ok");
    expect(view("granted", false, false).status.text).toMatch(/Push to a closed app is not set up for this account yet\./);
    expect(view("granted", true, true).rows.map((r) => [r.label, r.on])).toEqual([
      ["Needs you", true],
      ["Done", false],
    ]);
  });
});
