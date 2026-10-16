// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { BridgeProvider } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { DEMO_RECOVERY_KEY } from "@/bridge/adapters/demo/keyvault-setup";
import type { CoreClient } from "@/bridge/core";
import { noCore, testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { KeyvaultPage } from "@/components/keyvault/keyvault-page";

afterEach(cleanup);

function mount(core: CoreClient, adapter: DataAdapter = createDemoAdapter({ latencyMs: 0, stepMs: 2, noVault: true })) {
  render(
    <BridgeProvider adapter={adapter} core={core}>
      <KeyvaultPage />
    </BridgeProvider>,
  );
  return adapter;
}

describe("Keyvault: no vault yet", () => {
  for (const [label, core] of [
    ["with the app core", () => testCore()],
    ["without it", async () => noCore],
  ] as const) {
    it(`offers Set up Keyvault, then shows the recovery key once (${label})`, async () => {
      mount(await core());
      expect(await screen.findByText("No Keyvault yet")).toBeTruthy();
      expect(screen.getByText(/Set it up to save logins your agents can use/)).toBeTruthy();
      expect(screen.getByText("cua keyvault init")).toBeTruthy();
      // No search or item count before there is a vault.
      expect(screen.queryByLabelText("Search Keyvault")).toBeNull();
      fireEvent.click(screen.getByRole("button", { name: "Set up Keyvault" }));
      expect(await screen.findByText(DEMO_RECOVERY_KEY)).toBeTruthy();
      expect(screen.getByText("Save your recovery key")).toBeTruthy();
      await waitFor(() => expect(screen.queryByRole("button", { name: "Set up Keyvault" })).toBeNull());
      // The list is the core's; without it the page says so.
      if (label === "with the app core" && wasmBuilt) expect(screen.getByLabelText("Search Keyvault")).toBeTruthy();
      else expect(screen.getByText(/needs the app core/)).toBeTruthy();
      fireEvent.click(screen.getByRole("button", { name: "Done" }));
      expect(screen.queryByText(DEMO_RECOVERY_KEY)).toBeNull();
    });
  }

  it("says why when setup fails, and keeps the button", async () => {
    const base = createDemoAdapter({ latencyMs: 0, stepMs: 2, noVault: true });
    const adapter = Object.assign(Object.create(base) as DataAdapter, {
      call: ((op: string, args: unknown) =>
        op === "keyvault.setup"
          ? Promise.reject(new Error("This window only reads from cua for now."))
          : (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args)) as DataAdapter["call"],
    });
    mount(noCore, adapter);
    fireEvent.click(await screen.findByRole("button", { name: "Set up Keyvault" }));
    await waitFor(() => expect((screen.getByRole("button", { name: "Set up Keyvault" }) as HTMLButtonElement).disabled).toBe(false));
    expect(screen.queryByText("Save your recovery key")).toBeNull();
  });

  it("a ready vault has no setup panel", async () => {
    mount(await testCore(), createDemoAdapter({ latencyMs: 0 }));
    expect(await screen.findByText(wasmBuilt ? /Saved logins/ : /needs the app core/)).toBeTruthy();
    await waitFor(() => expect(screen.queryByLabelText(wasmBuilt ? "Search Keyvault" : "x") !== null || !wasmBuilt).toBe(true));
    expect(screen.queryByRole("button", { name: "Set up Keyvault" })).toBeNull();
  });
});
