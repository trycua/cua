// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import type { DataAdapter } from "../adapter";
import { HostError } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import { createWebkitAdapter } from "../adapters/webkit";
import type { Space } from "../contracts/spaces";
import { BridgeStore } from "../store";
import type { WebkitRequest } from "../webkit-protocol";
import type { HostWindow } from "../detect";
import { testCore, until, wasmBuilt } from "./testCore";

/** The demo host without `spaces.changed` (no event after a rename): the
 * store must read the list again itself. Records every call. */
function quietHost(calls: { op: string; args: unknown }[]): DataAdapter {
  const demo = createDemoAdapter({ latencyMs: 0 });
  return Object.assign(Object.create(demo), {
    subscribe: (l: Parameters<DataAdapter["subscribe"]>[0]) => demo.subscribe((e) => (e.type === "spaces.changed" ? undefined : l(e))),
    call: (op: string, args: unknown) => {
      calls.push({ op, args });
      return (demo.call as (op: string, args: unknown) => Promise<unknown>)(op, args);
    },
  }) as DataAdapter;
}

describe.skipIf(!wasmBuilt)("renaming a Space", () => {
  it("keeps its id, shows the new name everywhere, and says why a name is refused", async () => {
    const calls: { op: string; args: unknown }[] = [];
    const store = new BridgeStore(quietHost(calls), await testCore(), { tickMs: 5 });
    store.ensure("spaces");
    const spaces = () => store.get<Space[]>("spaces").data ?? [];
    const row = () => spaces().find((s) => s.id === "local:design-review");
    await until(() => expect(row()).toBeTruthy());
    const count = spaces().length;

    await store.renameSpace("local:design-review", "review-board");
    await until(() => expect(row()?.name).toBe("review-board"));
    expect(spaces()).toHaveLength(count);
    expect(calls.filter((c) => c.op === "spaces.rename")).toEqual([
      { op: "spaces.rename", args: { spaceId: "local:design-review", name: "review-board" } },
    ]);

    // Refused in plain words; nothing changes.
    await expect(store.renameSpace("local:design-review", "Review Board")).rejects.toThrow(/lowercase letters, digits and dashes/);
    const taken = spaces().find((s) => s.id !== "local:design-review")!;
    await expect(store.renameSpace("local:design-review", taken.name)).rejects.toThrow(/already called/);
    expect(row()?.name).toBe("review-board");
  });
});

describe("spaces.rename on the SwiftUI and Electron hosts", () => {
  it("sends {spaceId, name} and reads {} back", async () => {
    const sent: WebkitRequest[] = [];
    const win = {
      webkit: {
        messageHandlers: {
          cua: {
            postMessage: (m: unknown) => {
              const req = m as WebkitRequest;
              sent.push(req);
              return Promise.resolve({ id: req.id, ok: true, result: {} });
            },
          },
        },
      },
      addEventListener: () => {},
      removeEventListener: () => {},
      open: () => null,
    } as unknown as HostWindow;
    const adapter = createWebkitAdapter(win);
    await expect(adapter.call("spaces.rename", { spaceId: "relay:space-0a1b", name: "ci-runner" })).resolves.toBeNull();
    expect(sent.map((r) => [r.method, r.args])).toEqual([["spaces.rename", { spaceId: "relay:space-0a1b", name: "ci-runner" }]]);
  });

  it("passes the host's refusal through in its words", async () => {
    const win = {
      webkit: {
        messageHandlers: {
          cua: {
            postMessage: (m: unknown) => {
              const req = m as WebkitRequest;
              return Promise.resolve({
                id: req.id,
                ok: false,
                error: { message: "studio is shared with you. Only its owner can rename it.", code: "permission_denied" },
              });
            },
          },
        },
      },
      addEventListener: () => {},
      removeEventListener: () => {},
      open: () => null,
    } as unknown as HostWindow;
    const adapter = createWebkitAdapter(win);
    const e = await adapter.call("spaces.rename", { spaceId: "relay:space-0a1b", name: "mine" }).catch((x: unknown) => x);
    expect(e).toBeInstanceOf(HostError);
    expect((e as HostError).message).toBe("studio is shared with you. Only its owner can rename it.");
  });
});
