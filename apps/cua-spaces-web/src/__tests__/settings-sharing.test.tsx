// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, General, Sharing: "Let other machines create Spaces here" for this
// Mac, on by default. Turning it off and on again runs the host's own actions;
// there is no error and no command-line hint whichever way it is set.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
afterEach(cleanup);

describe.skipIf(!wasmBuilt)("Settings, Sharing", () => {
  it("has the one switch, on by default, and flips it with the host's actions", async () => {
    const actions: string[] = [];
    await mountApp("/settings/general", {
      failsCreates: false,
      listsGhost: false,
      intercept: (op, args) => {
        if (op === "host.action") {
          actions.push((args as { action: string }).action);
          return null;
        }
        return undefined;
      },
    });
    const sw = await waitFor(() => {
      const el = document.querySelector<HTMLElement>("#share-spaces");
      expect(el).not.toBeNull();
      return el!;
    });
    expect(document.body.textContent).toContain("Let other machines create Spaces here");
    expect(document.body.textContent).not.toMatch(/cua host config|nothing to share/);
    const on = (sw as HTMLInputElement).checked || sw.getAttribute("aria-checked") === "true" || sw.hasAttribute("data-checked");
    await act(async () => void fireEvent.click(sw));
    await waitFor(() => expect(actions.length).toBeGreaterThan(0));
    expect(actions[0]).toBe(on ? "stop-providing-spaces" : "provide-spaces");
  });

  it("shows the limits under the switch, with use next to each, and saves a changed number", async () => {
    const calls: [string, unknown][] = [];
    await mountApp("/settings/general", {
      failsCreates: false,
      listsGhost: false,
      intercept: (op, args) => {
        if (op === "host.limits") {
          calls.push([op, args]);
          return null;
        }
        return undefined;
      },
    });
    const rows = await waitFor(() => {
      const el = document.querySelector<HTMLElement>('[data-setting-row="limit-max-spaces"]');
      expect(el).not.toBeNull();
      return el!;
    });
    const text = document.body.textContent ?? "";
    for (const label of ["Most Spaces at once", "Most memory in total", "Most disk in total", "Longest a Space may run"]) expect(text).toContain(label);
    // Use sits next to the limit; this Mac's total next to memory and disk.
    expect(document.querySelector('[data-limit-use="max-spaces"]')?.textContent).toMatch(/of 4 Spaces/);
    expect(document.querySelector('[data-limit-use="max-memory"]')?.textContent).toMatch(/GB/);
    expect(text).not.toContain("\u00b7");
    expect(text).not.toMatch(/devices|computers/i);
    // No limit is an empty field.
    expect((document.querySelector('[data-setting-row="limit-max-memory"]') as HTMLInputElement).value).toBe("");
    // A number saves when the field is left.
    await act(async () => void fireEvent.change(rows, { target: { value: "6" } }));
    await act(async () => void fireEvent.blur(rows));
    await waitFor(() => expect(calls).toEqual([["host.limits", { maxSpaces: 6 }]]));
    // Leaving a field that was not changed saves nothing.
    const memory = document.querySelector<HTMLInputElement>('[data-setting-row="limit-max-memory"]')!;
    await act(async () => void fireEvent.blur(memory));
    expect(calls.length).toBe(1);
    // A number that is not a number is put back, not saved.
    await act(async () => void fireEvent.change(memory, { target: { value: "-3" } }));
    await act(async () => void fireEvent.blur(memory));
    expect(memory.value).toBe("");
    expect(calls.length).toBe(1);
  });
});
