// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// New Space's Kind row: the core's one quiet line under Kind says what the
// chosen kind is and why MicroVM is greyed out on this host, in view (not
// only in a hover tooltip), so a microVM that would fail is never offered.

import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetNewSpaceSession } from "@/bridge/new-space";
import { mountApp, stubBrowserApis } from "./app-harness";

beforeEach(resetNewSpaceSession);
afterEach(cleanup);
stubBrowserApis();

describe.skipIf(!wasmBuilt)("New Space's Kind line", () => {
  it("shows why MicroVM is greyed out on this host", { timeout: 20_000 }, async () => {
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
    const line = dialog.querySelector("[data-kind-help]");
    expect(line?.textContent).toMatch(/MicroVM needs /);
    // The same words the tile's tooltip gives, in one line.
    expect(line!.textContent!.toLowerCase()).toContain(tile.title.replace(/\.$/, "").toLowerCase());
  });
});
