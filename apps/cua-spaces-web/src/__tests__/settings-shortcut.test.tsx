// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, General, "Open Cua Spaces from any app" (the Mac hosts): Change
// records the next keys and the host takes them; Turn off frees the keys.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
afterEach(cleanup);

describe.skipIf(!wasmBuilt)("Settings, the global shortcut", () => {
  it("changes to the keys pressed, then turns off", async () => {
    const chosen: unknown[] = [];
    await mountApp("/settings/general", {
      failsCreates: false,
      listsGhost: false,
      intercept: (op, args) => {
        if (op === "settings.choose") chosen.push(args);
        return undefined;
      },
    });
    const label = () => document.querySelector("[data-shortcut-label]")?.textContent;
    await waitFor(() => expect(label()).toBe("⌘⇧Space"));

    await act(async () => void fireEvent.click(document.querySelector("[data-shortcut-change]")!));
    expect(label()).toBe("…");
    await act(async () => void fireEvent.keyDown(window, { key: "f", code: "KeyF", ctrlKey: true, altKey: true, metaKey: true }));
    await waitFor(() => expect(label()).toBe("⌃⌥⌘F"));
    expect(chosen).toContainEqual({ row: "global-shortcut", option: "Control+Alt+Command+F" });

    await act(async () => void fireEvent.click(document.querySelector("[data-shortcut-off]")!));
    await waitFor(() => expect(label()).toBe("Off"));
    expect(chosen).toContainEqual({ row: "global-shortcut", option: "off" });
    expect(document.querySelector("[data-global-shortcut]")?.getAttribute("data-global-shortcut")).toBe("off");
    expect(document.querySelector("[data-shortcut-off]")).toBeNull();
    expect(document.body.textContent).not.toContain("Another app is using");
  });
});
