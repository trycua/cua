// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Space's page in call view (the desktop leads) and thread view (the
// thread leads, the desktop in a floating mini-player): the hover-only
// toggle on the divider row, ⌘⇧T, the mini-player's expand arrows, and the
// view remembered per Space.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { DEMO_THREAD_SPACE } from "@/bridge/adapters/demo/thread";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();
beforeEach(() => localStorage.removeItem(`cua-spaces:layout:${DEMO_THREAD_SPACE}`));

const q = (s: string) => document.querySelector<HTMLElement>(s);
const path = `/spaces/${encodeURIComponent(DEMO_THREAD_SPACE)}`;

describe("call view and thread view", () => {
  it("toggles from the divider row, the mini-player and ⌘⇧T, and remembers the view per Space", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(q("[data-call-layout]")).not.toBeNull());
    // Call view: the shrink button sits at the left of the divider's row, shown only on hover or focus.
    const toggle = await waitFor(() => {
      const t = q("[data-call-divider-row] [data-layout-toggle='thread']");
      expect(t).not.toBeNull();
      return t!;
    });
    expect(toggle.getAttribute("aria-label")).toBe("Thread view");
    const holder = q("[data-call-divider-leading]")!;
    expect(holder.className).toContain("opacity-0");
    expect(holder.className).toContain("group-hover/divider:opacity-100");
    expect(holder.className).toContain("focus-within:opacity-100");
    // Pressing it never starts a divider drag.
    fireEvent.pointerDown(holder, { button: 0, pointerId: 1 });
    expect(q("[data-call-divider-bar]")!.className.split(" ")).not.toContain("bg-ring");
    // Nothing in the header for it.
    expect(q("[data-call-header] [data-layout-toggle]")).toBeNull();

    act(() => toggle.click());
    await waitFor(() => expect(q("[data-thread-layout]")).not.toBeNull());
    expect(q("[data-call-divider]")).toBeNull();
    expect(q("[data-mini-player]")).not.toBeNull();
    // The player floats over the thread only: the composer is outside its area.
    const area = q("[data-mini-player-area]")!;
    const composer = q("[data-composer]")!;
    expect(area.parentElement!.contains(composer)).toBe(false);
    expect(localStorage.getItem(`cua-spaces:layout:${DEMO_THREAD_SPACE}`)).toBe("thread");
    // In its default bottom-right corner, the thread keeps the player's height clear at its end,
    // so the newest message sits above it.
    const player = q("[data-mini-player]")!;
    expect(player.dataset.miniPlayer).toBe("br");
    const end = q("[data-thread] > div")!;
    await waitFor(() => expect(parseFloat(end.style.paddingBottom)).toBe(parseFloat(player.style.height) + 12 + 16));

    // Its expand arrows go back.
    act(() => q("[data-mini-player-expand]")!.click());
    await waitFor(() => expect(q("[data-call-divider]")).not.toBeNull());
    expect(localStorage.getItem(`cua-spaces:layout:${DEMO_THREAD_SPACE}`)).toBe("call");

    // ⌘⇧T (Ctrl+Shift+T elsewhere) toggles.
    act(() => void window.dispatchEvent(new KeyboardEvent("keydown", { key: "T", metaKey: true, ctrlKey: true, shiftKey: true })));
    await waitFor(() => expect(q("[data-thread-layout]")).not.toBeNull());

    // Remembered: the page opens again in thread view.
    cleanup();
    await mountApp(path, { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(q("[data-thread-layout]")).not.toBeNull());
  });
});
