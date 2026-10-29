// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Space another of your computers is creating (CUA-2349): the Mac mini
// publishes it through the relay, and this computer's Spaces page shows
// "Creating macOS on Mac mini… 40%" until it is ready.

import { act, cleanup, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { DEMO_CREATING_ELSEWHERE } from "@/bridge/adapters/demo";
import type { CreatingElsewhere } from "@/bridge/contracts/spaces";
import { creatingElsewhereLine } from "@/lib/spaces";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
afterEach(cleanup);

const WAIT = { timeout: 8000 };

const tile = () => document.querySelector("[data-creating-elsewhere]");

async function mount(creating: { current: CreatingElsewhere[] }, noSpaces = false) {
  return mountApp("/spaces", { failsCreates: false, listsGhost: false, demo: { noSpaces }, creatingElsewhere: () => creating.current });
}

describe("a Space another machine is creating", { timeout: 30_000 }, () => {
  it("shows under that computer's name with how far it got, and goes once it is done", async () => {
    const creating = { current: [...DEMO_CREATING_ELSEWHERE] };
    const { emit } = await mount(creating);
    await waitFor(() => expect(tile()).not.toBeNull(), WAIT);
    expect(tile()!.getAttribute("data-creating-elsewhere")).toBe("mac-mini");
    expect(tile()!.textContent).toContain("browser-qa");
    expect(tile()!.getAttribute("aria-label")).toBe("browser-qa: Creating macOS on Mac mini… 40%");
    expect(screen.getByText("Creating macOS on Mac mini…")).toBeTruthy();
    expect(tile()!.querySelector("[data-state-label]")!.textContent).toBe("40%");
    // Nothing to open yet: not a link.
    expect(tile()!.closest("a")).toBeNull();

    creating.current = [{ ...DEMO_CREATING_ELSEWHERE[0]!, phase: "booting", percent: 70 }];
    act(() => emit({ type: "spaces.changed" }));
    await waitFor(() => expect(tile()!.querySelector("[data-state-label]")!.textContent).toBe("70%"), WAIT);

    creating.current = [];
    act(() => emit({ type: "spaces.changed" }));
    await waitFor(() => expect(tile()).toBeNull(), WAIT);
  });

  it("is not the empty home when this machine has no Space yet", async () => {
    await mount({ current: [...DEMO_CREATING_ELSEWHERE] }, true);
    await waitFor(() => expect(tile()).not.toBeNull(), WAIT);
    expect(document.querySelector("[data-empty-home]")).toBeNull();
    expect(screen.queryByText("No Spaces yet")).toBeNull();
  });
});

describe("creatingElsewhereLine", () => {
  it("says what, where and how far in plain words", () => {
    expect(creatingElsewhereLine({ os: "macos", machineName: "Mac mini", percent: 40 })).toBe("Creating macOS on Mac mini… 40%");
    expect(creatingElsewhereLine({ os: "linux", machineName: "Studio", percent: 99.6 })).toBe("Creating Linux on Studio… 100%");
    expect(creatingElsewhereLine({ os: "", machineName: " ", percent: -3 })).toBe("Creating a Space on another machine… 0%");
  });
});
