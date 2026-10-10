// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { BridgeProvider, emptyHome, type EmptyHome as EmptyHomeView } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import type { CoreClient } from "@/bridge/core";
import { readNewSpaceSession, resetNewSpaceSession, wizardView } from "@/bridge/new-space";
import { noCore, testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { EmptyHome } from "@/components/empty-home";

beforeEach(resetNewSpaceSession);
afterEach(cleanup);

/** The chrome's empty-home words, as `window.chrome` gives them. */
const CHROME = {
  emptyTitle: "No Spaces yet",
  emptyDetail: "Choose a system for your first Space.",
  emptyAction: "Linux",
  emptyLinuxDetail: "About 4 minutes · 1.0–2.0 GB of disk",
  emptyMacosAction: "macOS",
  emptyMacosDetail: "About 19 GB download",
};

/** A core that answers only `window.chrome`. */
function stubCore(asked: unknown[] = []): CoreClient {
  return {
    status: "ready",
    methods: ["window.chrome"],
    tryCall: (method: string, args: unknown) => {
      asked.push([method, args]);
      return method === "window.chrome" ? CHROME : null;
    },
  } as unknown as CoreClient;
}

function mount(core: CoreClient, home: EmptyHomeView) {
  const base = createDemoAdapter({ latencyMs: 1, stepMs: 4, noSpaces: true });
  const created: unknown[] = [];
  // Records any create: a tile never starts one.
  const adapter = Object.assign(Object.create(base) as DataAdapter, {
    call: ((op: string, args: unknown) => {
      if (op === "spaces.create") created.push(args);
      return base.call(op as never, args as never);
    }) as DataAdapter["call"],
  });
  render(
    <BridgeProvider adapter={adapter} core={core} storeOptions={{ tickMs: 5 }}>
      <EmptyHome home={home} />
    </BridgeProvider>,
  );
  return { created };
}

describe("the empty home's words", () => {
  it("are the core's chrome, with macOS on Apple silicon Macs only", () => {
    const asked: unknown[] = [];
    const tiles = (arch: string | null, os: "macos" | "windows" | "linux") => emptyHome(stubCore(asked), arch, os)!.tiles.map((t) => [t.os, t.name, t.detail]);
    const linux = ["linux", "Linux", "About 4 minutes · 1.0–2.0 GB of disk"];
    const macos = ["macos", "macOS", "About 19 GB download"];
    expect(tiles("arm64", "macos")).toEqual([linux, macos]);
    expect(tiles("aarch64", "macos")).toEqual([linux, macos]);
    expect(tiles(null, "macos")).toEqual([linux, macos]);
    expect(tiles("amd64", "macos")).toEqual([linux]);
    expect(tiles("arm64", "windows")).toEqual([linux]);
    expect(tiles("amd64", "linux")).toEqual([linux]);
    expect(asked[0]).toEqual(["window.chrome", {}]);
    expect(emptyHome(stubCore(), null, "macos")).toMatchObject({ title: "No Spaces yet", detail: "Choose a system for your first Space." });
    expect(emptyHome(noCore, "arm64")).toBeNull();
  });

  it("draws each tile as a button named by its system and figures, and no New Space of its own", () => {
    mount(stubCore(), emptyHome(stubCore(), "arm64", "macos")!);
    expect(screen.getByRole("heading", { name: "No Spaces yet" })).toBeTruthy();
    expect(screen.getByText("Choose a system for your first Space.")).toBeTruthy();
    const linux = screen.getByRole("button", { name: "Linux, About 4 minutes · 1.0–2.0 GB of disk" });
    const macos = screen.getByRole("button", { name: "macOS, About 19 GB download" });
    // Focusable buttons with visible hover and focus states.
    linux.focus();
    expect(document.activeElement).toBe(linux);
    expect(linux.className).toContain("focus-visible:ring-2");
    expect(macos.className).toContain("hover:");
    // The page header's New Space is the one.
    expect(screen.getAllByRole("button")).toHaveLength(2);
  });
});

describe("a Spaces list that could not be read", () => {
  it("says so, with no system to choose", () => {
    const home = emptyHome(stubCore(), "arm64", "macos")!;
    const base = createDemoAdapter({ latencyMs: 1, stepMs: 4, noSpaces: true });
    render(
      <BridgeProvider adapter={base} core={stubCore()} storeOptions={{ tickMs: 5 }}>
        <EmptyHome home={home} unread />
      </BridgeProvider>,
    );
    expect(screen.getByRole("heading", { name: "Spaces could not be loaded" })).toBeTruthy();
    expect(screen.queryByText("Choose a system for your first Space.")).toBeNull();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });
});

describe.skipIf(!wasmBuilt)("the empty home (wasm core)", () => {
  it("shows the core's figures: Linux's measured time and disk, macOS's catalog download", async () => {
    const core = await testCore();
    expect(emptyHome(core, "arm64", "macos")!.tiles.map((t) => [t.name, t.detail])).toEqual([
      ["Linux", "About 1 minute · 3.2–7.1 GB of disk"],
      ["macOS", "About 22 GB download"],
    ]);
  });

  it("opens New Space with the tile's system chosen, on this machine, and never creates", async () => {
    const core = await testCore();
    const env = { defaultLocation: "local", cloudAvailable: false, localAvailable: true, maxCpus: 8 } as const;
    for (const [name, os] of [
      [/^Linux, /, "linux"],
      [/^macOS, /, "macos"],
    ] as const) {
      resetNewSpaceSession();
      const { created } = mount(core, emptyHome(core, "arm64", "macos")!);
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name }));
      });
      const s = readNewSpaceSession();
      expect(s.open).toBe(true);
      const view = wizardView(core, s.state!, env);
      expect(view.step).toBe(0);
      expect(view.image.os).toBe(os);
      expect(view.plan.placement).toBe("local");
      expect(created).toEqual([]);
      cleanup();
    }
  });
});
