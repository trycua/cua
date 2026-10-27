// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// New Space's Kind row has no line under it: a MicroVM this computer can't
// run is greyed out with no words there. Why it can't is told in Settings,
// Advanced, beside the runtimes (`settings::microvm_rows`), from the same
// runtime doctor the tile follows.

import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetNewSpaceSession } from "@/bridge/new-space";
import { mountApp, stubBrowserApis } from "./app-harness";

beforeEach(resetNewSpaceSession);
afterEach(cleanup);
stubBrowserApis();

describe.skipIf(!wasmBuilt)("MicroVM: greyed out in New Space, explained in Settings", () => {
  it("New Space greys MicroVM out with no line under Kind and no reason", { timeout: 20_000 }, async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    fireEvent.click(await screen.findByRole("button", { name: /New Space/ }));
    const dialog = await waitFor(
      () => {
        const d = document.querySelector<HTMLElement>("[data-new-space]");
        expect(d).not.toBeNull();
        return d!;
      },
      { timeout: 4000 },
    );
    fireEvent.click(await waitFor(() => dialog.querySelector<HTMLElement>("[data-advanced-toggle]")!));
    const tile = await waitFor(() => {
      const t = dialog.querySelector<HTMLButtonElement>("[data-tile='kind:microvm']");
      expect(t).not.toBeNull();
      return t!;
    });
    expect(tile.disabled).toBe(true);
    expect(tile.title).toBe("");
    expect(dialog.querySelector("[data-kind-help]")).toBeNull();
    expect(dialog.textContent).not.toMatch(/MicroVM needs|Needs macOS 26/);
  });

  it("Settings, Advanced says why MicroVMs don't run here", { timeout: 20_000 }, async () => {
    await mountApp("/settings/advanced", { failsCreates: false, listsGhost: false });
    const row = await waitFor(
      () => {
        const r = document.querySelector<HTMLElement>("[data-microvm-runtime]");
        expect(r).not.toBeNull();
        return r!;
      },
      { timeout: 8000 },
    );
    expect(row.querySelector("[data-row-label]")?.textContent).toBe("MicroVMs");
    expect(row.querySelector("[data-row-value]")?.textContent).toBe("Not available");
    expect(row.querySelector("[data-row-description]")?.textContent).toBe("MicroVM needs macOS 26.");
    expect(row.getAttribute("data-microvm-problem")).toBe("true");
  });
});
