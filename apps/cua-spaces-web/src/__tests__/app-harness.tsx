// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The whole app (its real routes, the wasm core) on the demo host, for tests
// of what a person sees across pages. The host fails creates on request,
// the way a macOS create on another Mac fails without Local Network access,
// and lists that machine's half-created Space as a record of its own.

import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { render } from "@testing-library/react";
import { afterEach, beforeEach } from "vitest";
import type { DataAdapter } from "@/bridge/adapter";
import type { MachinesView } from "@/bridge/contracts/host";
import type { CreatingElsewhere, ListStatus } from "@/bridge/contracts/spaces";
import type { HostEvent } from "@/bridge/protocol";
import { createDemoAdapter, type DemoOptions } from "@/bridge/adapters/demo";
import { BridgeProvider, useBridge, useMachines, useSpaces, type SpacesHook } from "@/bridge";
import { testCore } from "@/bridge/__tests__/testCore";
import { ToastProvider } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { routeTree } from "../routeTree.gen";

export const LOCAL_NETWORK = "Local Network access is not available: cua cannot reach the VM at 192.168.64.45:3211 (No route to host (os error 65))";
export const NAME = "e2e-1005-gamma-macos";
/** The machine's record of the Space it is creating. */
export const GHOST_ID = "relay:mac-mini/space-46e0d8654707222d";

/** jsdom has no matchMedia (the theme reads it) or ResizeObserver (the tiles' video slots). */
export function stubBrowserApis(): void {
  beforeEach(() => {
    window.matchMedia ??= ((query: string) => ({ matches: false, media: query, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false, onchange: null })) as unknown as typeof window.matchMedia;
    globalThis.ResizeObserver ??= class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  });
}

export interface TestHost {
  /** `spaces.create` fails with the Local Network error. */
  failsCreates: boolean;
  /** `spaces.list` includes the machine's record of the create. */
  listsGhost: boolean;
  /** `spaces.create` waits for this first (the create stays in flight). */
  hold?: Promise<void>;
  /** Rewrites the demo host's answer to `op` (extra rows, this machine's
   * enrollment). */
  answer?: (op: string, out: unknown) => unknown;
  /** Answers `op` itself instead of the demo host (undefined: the demo's). */
  intercept?: (op: string, args: unknown) => unknown;
  /** The Machines page as the host's core drew it (`machinesView`). */
  machinesView?: () => MachinesView | null;
  /** How far the host's list read got (`listStatus`); undefined: complete. */
  listStatus?: () => ListStatus;
  /** What the host's other computers are creating (`creatingElsewhere`). */
  creatingElsewhere?: () => CreatingElsewhere[];
  /** The demo host's own options (signed out, a first run). */
  demo?: DemoOptions;
}

/** The routers mounted by the running test: a navigation the app started (a
 * create that finishes sends the person to the new Space) may still be
 * loading when the test ends, and a router that settles after jsdom is gone
 * throws `window is not defined`. Each test waits for them to go idle first. */
const mounted: Array<{ state: { isLoading: boolean } }> = [];

afterEach(async () => {
  const routers = mounted.splice(0);
  const deadline = Date.now() + 3000;
  while (routers.some((r) => r.state.isLoading) && Date.now() < deadline) await new Promise((r) => setTimeout(r, 5));
});

export async function mountApp(path: string, host: TestHost = { failsCreates: true, listsGhost: true }) {
  const demo = createDemoAdapter({ latencyMs: 1, stepMs: 4, ...host.demo });
  // The page's listeners, so a test can push what the host would (`emit`).
  const listeners = new Set<(event: HostEvent) => void>();
  const adapter: DataAdapter = {
    mode: demo.mode,
    ...(host.machinesView ? { machinesView: host.machinesView } : {}),
    ...(host.listStatus ? { listStatus: host.listStatus } : {}),
    ...(host.creatingElsewhere ? { creatingElsewhere: host.creatingElsewhere } : {}),
    subscribe: (l) => {
      listeners.add(l);
      const off = demo.subscribe(l);
      return () => {
        listeners.delete(l);
        off();
      };
    },
    call: (async (op: string, args: never) => {
      if (op === "spaces.create") {
        await host.hold;
        if (host.failsCreates) throw new Error(LOCAL_NETWORK);
      }
      const own = host.intercept?.(op, args);
      if (own !== undefined) return own;
      const out = host.answer ? host.answer(op, await demo.call(op as never, args)) : await demo.call(op as never, args);
      if (op !== "spaces.list" || !host.listsGhost) return out;
      const ghost = { id: GHOST_ID, name: NAME, provider: "relay", spacesdVersion: "", features: [], reachable: false, host: "mac-mini", hostName: "Mac mini", addedAt: new Date(Date.now() + 9_000).toISOString() };
      return [...(out as unknown[]), ghost];
    }) as DataAdapter["call"],
  };
  const router = createRouter({ routeTree, history: createMemoryHistory({ initialEntries: [path] }) });
  mounted.push(router);
  const hooks = {} as { spaces: SpacesHook; machines: ReturnType<typeof useMachines>; core: ReturnType<typeof useBridge>["core"] };
  function Probe() {
    hooks.spaces = useSpaces();
    hooks.machines = useMachines();
    hooks.core = useBridge().core;
    return null;
  }
  // The store's create tick runs at the app's own pace (once a second), not
  // every few ms: while a create is in flight each tick re-renders the whole
  // app, and an `act` that ends while those renders keep coming never drains
  // its queue when a render takes longer than the tick (a loaded CI runner).
  render(
    <BridgeProvider adapter={adapter} core={await testCore()}>
      <TooltipProvider delay={500}>
        <ToastProvider>
          <Probe />
          <RouterProvider router={router} />
        </ToastProvider>
      </TooltipProvider>
    </BridgeProvider>,
  );
  const emit = (event: HostEvent) => listeners.forEach((l) => l(event));
  return { router, hooks, host, demo, emit };
}
