// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { IDLE_MS, PROMPTS_BEFORE_ASKING, recordPromptSent, resetStarNudge, STAR_URL, type StarNudgeHost } from "@/components/star-nudge";
import { ToastProvider, toastManager } from "@/components/ui/toast";

const TITLE = "Enjoying Cua Spaces?";

/** The nudges on screen (a closed toast may linger while it animates out). */
const nudges = () => [...document.querySelectorAll<HTMLElement>('[role="dialog"]')].filter((el) => !el.hasAttribute("data-ending-style") && el.textContent?.includes(TITLE));
const button = (selector: string) => nudges()[0]!.querySelector<HTMLElement>(selector)!;

function host() {
  const tracked: string[] = [];
  const opened: string[] = [];
  const h: StarNudgeHost = {
    track: (f) => void tracked.push(f),
    openExternal: async (url) => void opened.push(url),
  };
  return { h, tracked, opened };
}

/** Sends `n` prompts, then waits past the quiet moment. */
async function send(h: StarNudgeHost, n: number) {
  await act(async () => {
    for (let i = 0; i < n; i++) void recordPromptSent(h);
    await vi.advanceTimersByTimeAsync(IDLE_MS + 100);
  });
}

/** A new launch: this launch's memory goes, the page's storage stays. */
async function relaunch() {
  vi.resetModules();
  return import("@/components/star-nudge");
}

describe("star nudge", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetStarNudge();
    window.history.replaceState(null, "", "/");
    render(<ToastProvider>{null}</ToastProvider>);
  });
  afterEach(() => {
    act(() => toastManager.close());
    cleanup();
    vi.useRealTimers();
  });

  it("shows nothing before the 5th prompt", async () => {
    const { h, tracked } = host();
    await send(h, PROMPTS_BEFORE_ASKING - 1);
    expect(nudges()).toHaveLength(0);
    expect(tracked).toEqual([]);
  });

  it("shows once at the 5th prompt, after a quiet moment, and stays", async () => {
    const { h, tracked } = host();
    await send(h, PROMPTS_BEFORE_ASKING - 1);
    act(() => void recordPromptSent(h));
    // Still typing: not yet.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(IDLE_MS - 1000);
      window.dispatchEvent(new KeyboardEvent("keydown"));
      await vi.advanceTimersByTimeAsync(IDLE_MS - 1000);
    });
    expect(nudges()).toHaveLength(0);
    await act(async () => void (await vi.advanceTimersByTimeAsync(2000)));
    expect(nudges()).toHaveLength(1);
    expect(nudges()[0]!.textContent).toContain("Give us a star on GitHub.");
    // More prompts in the same launch don't add a second one.
    await send(h, 3);
    expect(nudges()).toHaveLength(1);
    expect(tracked).toEqual(["star_nudge_shown"]);
    // It doesn't go by itself.
    await act(async () => void (await vi.advanceTimersByTimeAsync(10 * 60_000)));
    expect(nudges()).toHaveLength(1);
  });

  it("opens the repository from its button and never shows again", async () => {
    const { h, tracked, opened } = host();
    await send(h, PROMPTS_BEFORE_ASKING);
    await act(async () => {
      fireEvent.click(button("[data-toast-action]"));
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(opened).toEqual([STAR_URL]);
    expect(tracked).toEqual(["star_nudge_shown", "star_nudge_click"]);
    expect(nudges()).toHaveLength(0);
    const next = await relaunch();
    await act(async () => {
      for (let i = 0; i < 10; i++) void next.recordPromptSent(h);
      await vi.advanceTimersByTimeAsync(IDLE_MS + 100);
    });
    expect(nudges()).toHaveLength(0);
    expect(tracked).toEqual(["star_nudge_shown", "star_nudge_click"]);
  });

  it("never shows again after it is dismissed", async () => {
    const { h, tracked } = host();
    await send(h, PROMPTS_BEFORE_ASKING);
    await act(async () => {
      fireEvent.click(button('[aria-label="Dismiss"]'));
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(tracked).toEqual(["star_nudge_shown", "star_nudge_dismiss"]);
    expect(nudges()).toHaveLength(0);
    const next = await relaunch();
    await act(async () => {
      for (let i = 0; i < 10; i++) void next.recordPromptSent(h);
      await vi.advanceTimersByTimeAsync(IDLE_MS + 100);
    });
    expect(nudges()).toHaveLength(0);
    expect(tracked).toEqual(["star_nudge_shown", "star_nudge_dismiss"]);
  });

  it("waits out onboarding and asks at the next prompt", async () => {
    const { h, tracked } = host();
    window.history.replaceState(null, "", "/onboarding");
    await send(h, PROMPTS_BEFORE_ASKING);
    expect(nudges()).toHaveLength(0);
    window.history.replaceState(null, "", "/spaces");
    await send(h, 1);
    expect(nudges()).toHaveLength(1);
    expect(tracked).toEqual(["star_nudge_shown"]);
  });
});
