// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The words and states the SwiftUI app shows come from the
// core here too: a creating card's line, a Space's status word, the GPU
// checkbox's name, and the chosen System tile drawn filled.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { BridgeProvider } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { rowsToSpaces } from "@/bridge/derive";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { StateLabel } from "@/components/state-dot";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

describe.skipIf(!wasmBuilt)("the core's words and states (wasm core)", () => {
  it("says a Space's status as the core does: Suspended, not Stopped", async () => {
    const core = await testCore();
    const [space] = rowsToSpaces(core, [{ id: "local:fbd1", name: "fbd1", provider: "local", os: "linux", spacesdVersion: "0.6.0", features: [], reachable: false }], 0);
    expect(space!.status).toBe("suspended");
    const { container } = render(
      <BridgeProvider adapter={createDemoAdapter({ latencyMs: 0 })} core={core}>
        <StateLabel state="stopped" space={space} />
      </BridgeProvider>,
    );
    expect(container.textContent).toBe("Suspended");
  });

  it("shows a creating card's line as the core's pending row says it", async () => {
    let release!: () => void;
    const hold = new Promise<void>((r) => (release = r));
    const { hooks } = await mountApp("/spaces", { failsCreates: false, listsGhost: false, hold });
    await waitFor(() => expect(hooks.spaces.data?.length).toBeGreaterThan(0));
    act(() => void hooks.spaces.createSpace({ image: "ghcr.io/trycua/linux:24.04-slim", os: "linux" }).catch(() => {}));
    const row = await waitFor(() => {
      const r = hooks.spaces.data!.find((s) => s.id.startsWith("pending:"));
      expect(r).toBeDefined();
      return r!;
    });
    const card = await waitFor(() => {
      const c = document.querySelector(`[data-space-id="${row.id}"]`);
      expect(c).not.toBeNull();
      return c!;
    });
    expect(row.detail).toMatch(/^This Mac · /);
    expect(card.textContent).toContain(row.detail);
    release();
  });

  it("draws the chosen System tile filled", async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    fireEvent.click(await screen.findByRole("button", { name: "New Space" }));
    const pressed = await waitFor(() => {
      const t = document.querySelector('[data-tile^="os:"][aria-pressed="true"]');
      expect(t).not.toBeNull();
      return t!;
    });
    expect(pressed.className).toContain("aria-pressed:bg-primary");
  });
});
