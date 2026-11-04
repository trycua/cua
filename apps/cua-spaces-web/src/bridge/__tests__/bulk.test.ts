// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import type { DataAdapter } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import { sharingOf, spaceToRow } from "../adapters/webkit";
import type { Space } from "../contracts/spaces";
import { rowsToSpaces } from "../derive";
import { BridgeStore } from "../store";
import { testCore, until, wasmBuilt } from "./testCore";

const NOW = Date.UTC(2026, 9, 24, 12);

describe.skipIf(!wasmBuilt)("sharing facts on every Space", () => {
  it("the demo has one of yours shared with five, one each from Sam and Priya, and a team's", async () => {
    const core = await testCore();
    const adapter = createDemoAdapter({ latencyMs: 0 });
    const spaces = rowsToSpaces(core, await adapter.call("spaces.list", {}), NOW);
    const by = (id: string) => spaces.find((s) => s.id === id)!.access!;
    const mine = by("local:design-review");
    expect(mine).toMatchObject({ kind: "mine", role: "owner", place: "This Mac", canDelete: true, canShare: true, canLeave: false });
    expect(mine.people.map((p) => [p.who, p.role])).toEqual([
      ["sam@example.com", "editor"],
      ["priya@example.com", "editor"],
      ["alex@example.com", "viewer"],
      ["jordan@example.com", "viewer"],
      ["morgan@example.com", "viewer"],
    ]);
    expect(by("relay:space-sam-qa-shared")).toMatchObject({ kind: "withMe", role: "editor", ownerName: "Sam Rivera", place: "Sam's Mac mini", canPower: true, canLeave: true, canDelete: false });
    expect(by("relay:space-priya-checkout")).toMatchObject({ kind: "withMe", role: "viewer", place: "Priya's PC", canPower: false, canTeleport: false, canLeave: true });
    expect(by("relay:space-qa-regression")).toMatchObject({ kind: "team", teamName: "QA team", place: "QA team", canLeave: true, canDelete: false });
    // A Space without facts is yours.
    expect(by("local:agent-sandbox")).toMatchObject({ kind: "mine", people: [] });
  });

  it("survive the SwiftUI host's round trip (`AppSpace` back to a row)", async () => {
    const core = await testCore();
    const adapter = createDemoAdapter({ latencyMs: 0 });
    const first = rowsToSpaces(core, await adapter.call("spaces.list", {}), NOW);
    const again = rowsToSpaces(
      core,
      first.map((s) => spaceToRow(s as never)),
      NOW,
    );
    for (const s of first) expect(again.find((x) => x.id === s.id)?.access, s.id).toEqual(s.access);
    expect(sharingOf({ kind: "mine", role: "owner", place: "This Mac", people: [], canPower: true, canTeleport: true, canShare: true, canRename: true, canDelete: true, canLeave: false })).toBeUndefined();
  });
});

/** The demo host, its bulk calls held until `release`. */
function heldHost(): { adapter: DataAdapter; release: () => void } {
  const demo = createDemoAdapter({ latencyMs: 0, stepMs: 1 });
  let release!: () => void;
  const held = new Promise<void>((r) => (release = r));
  const adapter = Object.assign(Object.create(demo), {
    call: async (op: string, args: unknown) => {
      if (op === "spaces.deleteMany" || op === "spaces.leave") await held;
      return demo.call(op as never, args as never);
    },
  }) as DataAdapter;
  return { adapter, release };
}

describe.skipIf(!wasmBuilt)("deleting and leaving several Spaces", () => {
  it("marks each row deleting until its result, and answers one result per id", async () => {
    const { adapter, release } = heldHost();
    const store = new BridgeStore(adapter, await testCore(), { tickMs: 5 });
    store.ensure("spaces");
    const rows = () => store.get<Space[]>("spaces").data ?? [];
    await until(() => expect(rows().length).toBeGreaterThan(6));
    const ids = ["local:agent-sandbox", "relay:linux-box/windows-legacy", "relay:space-sam-qa-shared"];
    const running = store.deleteSpaces(ids);
    await until(() => expect(ids.every((id) => rows().find((s) => s.id === id)?.status === "deleting")).toBe(true));
    release();
    const results = await running;
    expect(results.map((r) => [r.id, r.ok])).toEqual([
      ["local:agent-sandbox", true],
      ["relay:linux-box/windows-legacy", true],
      ["relay:space-sam-qa-shared", false],
    ]);
    expect(results[2]!.error).toBe("Couldn't delete qa-shared: it isn't yours.");
    await until(() => expect(rows().some((s) => s.id === "local:agent-sandbox")).toBe(false));
    // The one that failed is back as it was.
    expect(rows().find((s) => s.id === "relay:space-sam-qa-shared")?.status).not.toBe("deleting");
  });

  it("leaves Spaces shared with you and refuses your own", async () => {
    const store = new BridgeStore(createDemoAdapter({ latencyMs: 0, stepMs: 1 }), await testCore(), { tickMs: 5 });
    store.ensure("spaces");
    const rows = () => store.get<Space[]>("spaces").data ?? [];
    await until(() => expect(rows().length).toBeGreaterThan(6));
    const results = await store.leaveSpaces(["relay:space-priya-checkout", "local:design-review"]);
    expect(results).toEqual([
      { id: "relay:space-priya-checkout", ok: true },
      { id: "local:design-review", ok: false, error: "Couldn't leave design-review: it's yours; delete it instead." },
    ]);
    await until(() => expect(rows().some((s) => s.id === "relay:space-priya-checkout")).toBe(false));
    expect(rows().find((s) => s.id === "local:design-review")?.status).not.toBe("deleting");
  });

  it("answers every id when the host call fails, and nothing for no ids", async () => {
    const demo = createDemoAdapter({ latencyMs: 0 });
    const failing = Object.assign(Object.create(demo), {
      call: async (op: string, args: unknown) => {
        if (op === "spaces.deleteMany") throw new Error("the app went away");
        return demo.call(op as never, args as never);
      },
    }) as DataAdapter;
    const store = new BridgeStore(failing, await testCore(), { tickMs: 5 });
    expect(await store.deleteSpaces([])).toEqual([]);
    expect(await store.deleteSpaces(["local:design-review"])).toEqual([{ id: "local:design-review", ok: false, error: "the app went away" }]);
  });
});
