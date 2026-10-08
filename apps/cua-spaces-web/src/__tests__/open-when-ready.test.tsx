// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// New Space's "Open when ready", as the SwiftUI app's (`AppModel.create`
// selects the new row when `plan.openDesktop`): Create Space opens the new
// Space's page at once with its progress, and once it is ready the page is
// the Space's own, its desktop connecting; without it the Spaces page shows
// the new tile.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

/** Through the wizard to its summary, with "Open when ready" as given. */
async function createFromWizard(openWhenReady: boolean) {
  fireEvent.click(await screen.findByRole("button", { name: /New Space/ }));
  const dialog = await waitFor(
    () => {
      const d = document.querySelector<HTMLElement>("[data-new-space]");
      expect(d).not.toBeNull();
      return d!;
    },
    { timeout: 4000 },
  );
  for (let step = 0; step < 3; step++) {
    const next = await waitFor(() => {
      const b = dialog.querySelector<HTMLButtonElement>("[data-wizard-primary]")!;
      expect(b.disabled).toBe(false);
      return b;
    });
    if (step === 2) {
      const box = dialog.querySelector<HTMLElement>("[data-open-when-ready]")!;
      if ((box.getAttribute("aria-checked") === "true") !== openWhenReady) fireEvent.click(box);
      await waitFor(() => expect(box.getAttribute("aria-checked")).toBe(String(openWhenReady)));
    }
    fireEvent.click(next);
  }
  const create = await waitFor(() => {
    const b = dialog.querySelector<HTMLButtonElement>("[data-wizard-primary]")!;
    expect(b.textContent).toMatch(/Create Space/);
    return b;
  });
  // The router's navigation is a transition: let it commit inside act.
  await act(async () => {
    create.click();
    await new Promise((r) => setTimeout(r, 50));
  });
}

describe.skipIf(!wasmBuilt)("New Space, Open when ready", () => {
  it("opens the new Space's page at once, and follows it to the Space it became", { timeout: 20_000 }, async () => {
    let release = () => {};
    const hold = new Promise<void>((r) => (release = r));
    const { router } = await mountApp("/spaces", { failsCreates: false, listsGhost: false, hold });
    await createFromWizard(true);
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/spaces\/pending%3A|^\/spaces\/pending:/));
    // The page's own detail (its route loads on first use), with the create's progress.
    await waitFor(() => expect(document.querySelector("[data-detail-title]")).not.toBeNull(), { timeout: 8000 });
    expect((await screen.findAllByText(/Creating|Preparing|Pulling|Starting/)).length).toBeGreaterThan(0);
    await act(async () => {
      release();
      await new Promise((r) => setTimeout(r, 50));
    });
    await waitFor(() => expect(router.state.location.pathname).not.toMatch(/pending/), { timeout: 5000 });
    expect(router.state.location.pathname).toMatch(/^\/spaces\/local/);
  });

  it("stays on the Spaces page without it", { timeout: 20_000 }, async () => {
    const { router } = await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    await createFromWizard(false);
    await waitFor(() => expect(router.state.location.pathname).toBe("/spaces"));
  });
});
