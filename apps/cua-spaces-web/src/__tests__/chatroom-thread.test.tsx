// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The thread as a chatroom (CUA-2640): streaming text arrives as whole-text snapshots drawn at most
// every LIVE_COALESCE_MS, "Working for 1m 05s" ticks by writing its own text (no render per tick), a
// reply quotes what it answers only when that is not right above it, and an error is never cut.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { Profiler } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ThreadItem } from "@/bridge";
import { ToastProvider, toast } from "@/components/ui/toast";
import { aboveSeq } from "@/components/space-thread/thread-view";
import { elapsedText, WorkingFor } from "@/components/space-thread/live-row";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("the working row's clock", () => {
  it("says how long, in the shortest words", () => {
    expect(elapsedText(0)).toBe("0s");
    expect(elapsedText(12_000)).toBe("12s");
    expect(elapsedText(65_000)).toBe("1m 05s");
    expect(elapsedText(3_725_000)).toBe("1h 02m");
  });

  it("ticks by writing its own text, without rendering again", () => {
    vi.useFakeTimers();
    let t = 1_000_000;
    const now = () => t;
    const renders: string[] = [];
    render(
      <Profiler id="clock" onRender={(_, phase) => renders.push(phase)}>
        <WorkingFor sinceMs={t - 65_000} now={now} />
      </Profiler>,
    );
    const el = document.querySelector("[data-live-elapsed]")!;
    expect(el.textContent).toBe("Working for 1m 05s");
    for (let i = 0; i < 3; i++) {
      t += 1000;
      act(() => {
        vi.advanceTimersByTime(1000);
      });
    }
    expect(el.textContent).toBe("Working for 1m 08s");
    // Mounted once; three seconds passed and React drew nothing more.
    expect(renders).toEqual(["mount"]);
  });
});

describe("a reply and what it answers", () => {
  const item = (seq: number, type: ThreadItem["type"] = "message") => ({ type, key: `k${seq}`, entry: { seq } }) as unknown as ThreadItem;

  it("knows the entry right above it, past notes and the unread divider", () => {
    const items = [item(1), item(2, "system"), { type: "unread", key: "unread" } as ThreadItem, item(4)];
    expect(aboveSeq(items, 0)).toBeNull();
    expect(aboveSeq(items, 3)).toBe(1);
  });
});

describe("an error is never cut", () => {
  beforeEach(() => vi.useRealTimers());

  it("an error toast shows every word of a long message, selectable, with Copy", async () => {
    const long = `Failed to authenticate. ${"API Error: 401 OAuth access token is invalid. ".repeat(12)}Refresh your existing token.`;
    render(<ToastProvider>{null}</ToastProvider>);
    act(() => {
      toast("Couldn't start the agent", { description: long, type: "error" });
    });
    const body = await screen.findByText((_, el) => el?.textContent === long && el.className.includes("select-text"));
    // Wrapped, not clipped: no ellipsis class on it, and the full text is in the DOM.
    expect(body.className).not.toContain("truncate");
    expect(body.className).toContain("break-words");
    const copy = document.querySelector<HTMLButtonElement>("[data-toast-copy]")!;
    const write = vi.fn(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText: write } });
    fireEvent.click(copy);
    expect(write).toHaveBeenCalledWith(long);
  });
});

describe("streaming snapshots", () => {
  it("draws the latest snapshot at most every LIVE_COALESCE_MS, and a new stage at once", async () => {
    const { BridgeProvider } = await import("@/bridge");
    const { useSpaceThread, LIVE_COALESCE_MS } = await import("@/bridge/thread");
    const { noCore } = await import("@/bridge/__tests__/testCore");
    const snapshot = (stage: string, text?: string) => ({ key: `ada=${stage}#${text?.length ?? 0}`, agents: [{ agent: "ada", stage, ...(text ? { text } : {}) }] });
    const page = (live: ReturnType<typeof snapshot>) => ({ entries: [], cursor: 1, caughtUp: true, unread: 0, readSeq: 1, me: "me", hasMoreBefore: false, role: null, live });
    // The follow answers at once, three snapshots of one stage (what is said grows), then waits.
    const script = [page(snapshot("replying", "Hel")), page(snapshot("replying", "Hello")), page(snapshot("replying", "Hello wor"))];
    let follows = 0;
    const adapter = {
      mode: "demo",
      subscribe: () => () => {},
      call: async (op: string, args: Record<string, unknown>) => {
        if (op === "thread.read" && args.followMs) {
          const next = script[follows++];
          // Each snapshot a few ms after the last, well inside one window.
          return next ? new Promise((r) => setTimeout(() => r(next), 5)) : new Promise(() => {});
        }
        if (op === "thread.read") return page({ key: "", agents: [] });
        if (op === "thread.members") return { members: [] };
        return [];
      },
    } as never;
    const env = globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean };
    const was = env.IS_REACT_ACT_ENVIRONMENT;
    env.IS_REACT_ACT_ENVIRONMENT = false;
    const drawn: (string | undefined)[] = [];
    function Probe() {
      const t = useSpaceThread("local:dev");
      drawn.push(t.live[0]?.text);
      return null;
    }
    render(
      <BridgeProvider adapter={adapter} core={noCore}>
        <Probe />
      </BridgeProvider>,
    );
    // Outside act(), so every render React makes is a render the page would draw.
    await new Promise((r) => setTimeout(r, LIVE_COALESCE_MS * 4));
    const texts = drawn.filter((t, i) => t !== undefined && drawn[i - 1] !== t);
    // The stage appearing shows at once ("Hel"); the two snapshots that followed within the window were
    // drawn as one: the latest, whole.
    expect(texts[0]).toBe("Hel");
    expect(texts).not.toContain("Hello");
    expect(texts.at(-1)).toBe("Hello wor");
    env.IS_REACT_ACT_ENVIRONMENT = was;
  });
});
