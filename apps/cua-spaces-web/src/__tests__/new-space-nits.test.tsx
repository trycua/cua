// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// New Space (CUA-1641): a slider's number can be clicked and typed, within
// the slider's own limits and with its words when outside them; and a Space
// left unnamed shows the name it will get (the Name placeholder, the
// Summary, the row while it is created), the one the create uses.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetNewSpaceSession } from "@/bridge/new-space";
import { mountApp, stubBrowserApis } from "./app-harness";

beforeEach(resetNewSpaceSession);
afterEach(cleanup);
stubBrowserApis();

async function openWizard(): Promise<HTMLElement> {
  fireEvent.click(await screen.findByRole("button", { name: /New Space/ }));
  return waitFor(
    () => {
      const d = document.querySelector<HTMLElement>("[data-new-space]");
      expect(d).not.toBeNull();
      return d!;
    },
    { timeout: 4000 },
  );
}

async function next(dialog: HTMLElement) {
  const b = await waitFor(() => {
    const b = dialog.querySelector<HTMLButtonElement>("[data-wizard-primary]")!;
    expect(b.disabled).toBe(false);
    return b;
  });
  fireEvent.click(b);
}

const input = (dialog: HTMLElement, id: string) => dialog.querySelector<HTMLInputElement>(`[data-value-input="${id}"]`);

describe.skipIf(!wasmBuilt)("New Space nits", () => {
  it("types a slider's value within its limits, and says why outside them", { timeout: 20_000 }, async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    const dialog = await openWizard();
    await next(dialog);
    await waitFor(() => expect(dialog.getAttribute("data-step")).toBe("1"));

    // Clicking the number opens a field with the value.
    fireEvent.click(dialog.querySelector('[data-value-text="cpus"]')!);
    const cpus = await waitFor(() => {
      const f = input(dialog, "cpus");
      expect(f).not.toBeNull();
      return f!;
    });
    const max = Number(dialog.querySelector<HTMLInputElement>("#ns-cpus")!.max);
    // Outside the limits: the slider's words, Continue waits.
    fireEvent.change(cpus, { target: { value: String(max + 1) } });
    await waitFor(() => expect(dialog.querySelector('[data-field-error="cpus"]')?.textContent).toBe(`Choose 1 to ${max} cores.`));
    expect(cpus.getAttribute("aria-invalid")).toBe("true");
    expect(dialog.querySelector<HTMLButtonElement>("[data-wizard-primary]")!.disabled).toBe(true);
    fireEvent.keyDown(cpus, { key: "Enter" });
    expect(input(dialog, "cpus")).not.toBeNull();
    // Escape keeps the slider's value and the sheet.
    fireEvent.keyDown(input(dialog, "cpus")!, { key: "Escape" });
    await waitFor(() => expect(input(dialog, "cpus")).toBeNull());
    expect(document.querySelector("[data-new-space]")).not.toBeNull();

    // Within them: Return uses it.
    fireEvent.click(dialog.querySelector('[data-value-text="memory"]')!);
    const memory = await waitFor(() => input(dialog, "memory")!);
    fireEvent.change(memory, { target: { value: "6 GB" } });
    fireEvent.keyDown(memory, { key: "Enter" });
    await waitFor(() => expect(dialog.querySelector('[data-value-text="memory"]')?.textContent).toBe("6 GB"));
    expect(dialog.querySelector<HTMLInputElement>("#ns-memory")!.value).toBe("6");
  });

  it("shows the generated name as the placeholder, in the Summary and on the new row", { timeout: 20_000 }, async () => {
    let release = () => {};
    const hold = new Promise<void>((r) => (release = r));
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, hold });
    const dialog = await openWizard();
    await next(dialog);
    await next(dialog);
    const name = await waitFor(() => dialog.querySelector<HTMLInputElement>("[data-name-input]")!);
    const generated = name.placeholder;
    expect(generated).toMatch(/^space-[0-9a-f]{10}$/);
    await next(dialog);
    await waitFor(() => expect(dialog.textContent).toContain(`Name${generated}`));
    // The create uses it: the row while it is created carries the name.
    const create = await waitFor(() => {
      const b = dialog.querySelector<HTMLButtonElement>("[data-wizard-primary]")!;
      expect(b.textContent).toMatch(/Create Space/);
      return b;
    });
    await act(async () => {
      create.click();
      await new Promise((r) => setTimeout(r, 50));
    });
    await waitFor(() => expect(screen.getAllByText(generated).length).toBeGreaterThan(0), { timeout: 8000 });
    await act(async () => {
      release();
      await new Promise((r) => setTimeout(r, 50));
    });
  });
});
