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
});
