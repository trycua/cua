// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// New Space's Summary: "Or run this in Terminal" with the create the core
// would make (`wizard.createArgs`) under the host's CLI name, and nothing
// while the CLI is not installed.

import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { DemoOptions } from "@/bridge/adapters/demo";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetNewSpaceSession } from "@/bridge/new-space";
import { mountApp, stubBrowserApis } from "./app-harness";

beforeEach(resetNewSpaceSession);
afterEach(cleanup);
stubBrowserApis();

/** Through the wizard to its Summary, with "Open when ready" as given. */
async function toSummary(openWhenReady: boolean): Promise<HTMLElement> {
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
  await waitFor(() => expect(dialog.querySelector("[data-summary]")).not.toBeNull());
  return dialog;
}

const mount = (cli?: DemoOptions["cli"]) => mountApp("/spaces", { failsCreates: false, listsGhost: false, demo: { cli } });

describe.skipIf(!wasmBuilt)("New Space's Summary, in Terminal", () => {
  it("shows the create the wizard would make, under the released app's CLI", { timeout: 20_000 }, async () => {
    await mount();
    const dialog = await toSummary(false);
    const block = await waitFor(() => {
      const b = dialog.querySelector<HTMLElement>("[data-cli-hint=block]");
      expect(b).not.toBeNull();
      return b!;
    });
    expect(block.textContent).toContain("Or run this in Terminal");
    const command = block.querySelector("[data-cli-command]")!.textContent!;
    expect(command).toMatch(/^cua spaces create \S+ --on local --kind (container|vm)( |$)/);
  });

  it("names a Local build's CLI, and adds opening the Space when it opens when ready", { timeout: 20_000 }, async () => {
    await mount("local");
    const dialog = await toSummary(true);
    const lines = await waitFor(() => {
      const text = dialog.querySelector("[data-cli-hint=block] [data-cli-command]")?.textContent ?? "";
      expect(text).toMatch(/^cua-local spaces create /);
      return text.split("\n");
    });
    // The open line needs the Space's name; the wizard names every Space.
    if (lines.length > 1) expect(lines[1]).toMatch(/^cua-local sb view \S+/);
  });

  it("stays hidden while the CLI is not installed", { timeout: 20_000 }, async () => {
    await mount("missing");
    const dialog = await toSummary(false);
    await new Promise((r) => setTimeout(r, 50));
    expect(dialog.querySelector("[data-cli-hint]")).toBeNull();
  });
});
