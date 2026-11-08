// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Space another of your computers is creating (CUA-2349): the Mac mini
// publishes it through the relay, and the list of your computers shows it
// under the Mac mini, Getting ready with how far it got, until it is listed.

import { act, cleanup, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { DEMO_CREATING_ELSEWHERE } from "@/bridge/adapters/demo";
import type { CreatingElsewhere } from "@/bridge/contracts/spaces";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
afterEach(cleanup);

const WAIT = { timeout: 8000 };

const tile = () => document.querySelector("[data-creating-elsewhere]");

async function mount(creating: { current: CreatingElsewhere[] }, noSpaces = false) {
  return mountApp("/spaces", { failsCreates: false, listsGhost: false, demo: { noSpaces }, creatingElsewhere: () => creating.current });
}

describe("a Space another computer is creating", { timeout: 30_000 }, () => {
  it("shows under that computer's name with how far it got, and goes once it is done", async () => {
    const creating = { current: [...DEMO_CREATING_ELSEWHERE] };
    const { emit } = await mount(creating);
    await waitFor(() => expect(tile()).not.toBeNull(), WAIT);
    expect(tile()!.getAttribute("data-creating-elsewhere")).toBe("mac-mini");
    expect(tile()!.textContent).toContain("browser-qa");
    expect(tile()!.getAttribute("aria-label")).toBe("browser-qa: Getting ready on Mac mini, 40%");
    expect(tile()!.textContent).toContain("Getting ready");
    expect(tile()!.querySelector("[data-state-label]")!.textContent).toBe("40%");
    // Nothing to open yet: no link, no button.
    expect(tile()!.querySelector("a, button")).toBeNull();

    creating.current = [{ ...DEMO_CREATING_ELSEWHERE[0]!, phase: "booting", percent: 70 }];
    act(() => emit({ type: "spaces.changed" }));
    await waitFor(() => expect(tile()!.querySelector("[data-state-label]")!.textContent).toBe("70%"), WAIT);

    creating.current = [];
    act(() => emit({ type: "spaces.changed" }));
    await waitFor(() => expect(tile()).toBeNull(), WAIT);
  });

  it("is not the empty list when this computer has no Space yet", async () => {
    await mount({ current: [...DEMO_CREATING_ELSEWHERE] }, true);
    await waitFor(() => expect(tile()).not.toBeNull(), WAIT);
    expect(document.querySelector("[data-tree-empty]")).toBeNull();
  });
});
