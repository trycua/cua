// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Talking to agents without typing @ every time (CUA-2757): the Reply button
// on an agent's message, the sticky "talking to" chip in the composer, and
// `@everyone`. The page names the agent in the text it posts.

import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Composer, type ComposerHandle } from "@/components/space-thread/composer";
import { DEMO_THREAD_SPACE } from "@/bridge/adapters/demo/thread";
import { pinAgent, useAddressing } from "@/lib/addressing";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
const path = `/spaces/${encodeURIComponent(DEMO_THREAD_SPACE)}`;

async function composer() {
  return waitFor(() => {
    const el = q("[data-composer]") as HTMLTextAreaElement | null;
    expect(el?.disabled).toBe(false);
    return el!;
  });
}

describe("Reply on an agent's message", () => {
  it("is on agents' messages only, named for screen readers, and fills the box once without touching the text", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    const box = await composer();
    await waitFor(() => expect(qa("[data-reply]").length).toBeGreaterThan(0));
    const buttons = qa("[data-reply]");
    for (const b of buttons) {
      expect(b.getAttribute("aria-label")).toMatch(/^Reply to /);
      // Only agents' rows have one.
      expect(b.closest("[data-entry]")?.querySelector("[data-person-name]")).toBeNull();
    }
    // Hidden until hover or focus, but there for keyboards and touch.
    expect(buttons[0]!.className).toContain("group-focus-within/msg:opacity-100");
    expect(buttons[0]!.className).toContain("max-md:opacity-100");

    // Clicking a message's text does not reply, fill the box or take focus.
    const row = buttons[0]!.closest<HTMLElement>("[data-entry]")!;
    await act(async () => {
      fireEvent.click(row.querySelector("p, div.select-text") ?? row);
    });
    expect(box.value).toBe("");

    const handle = buttons[0]!.dataset.reply!;
    await act(async () => {
      fireEvent.click(buttons[0]!);
    });
    expect(box.value).toBe(`@${handle} `);
    expect(document.activeElement).toBe(box);
    expect(box.selectionStart).toBe(box.value.length);
    // Once: replying again keeps what is there.
    await act(async () => {
      fireEvent.change(box, { target: { value: `@${handle} half a sentence`, selectionStart: 20 } });
      fireEvent.click(buttons[0]!);
    });
    expect(box.value).toBe(`@${handle} half a sentence`);
  });

  it("replies with the R key on a focused message", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    const box = await composer();
    await waitFor(() => expect(qa("[data-reply]").length).toBeGreaterThan(0));
    const button = qa("[data-reply]")[0]!;
    button.focus();
    await act(async () => {
      fireEvent.keyDown(button, { key: "r" });
    });
    expect(box.value).toBe(`@${button.dataset.reply} `);
    expect(document.activeElement).toBe(box);
  });

  it("pins the agent as a chip once the reply is sent, and the next message needs no @", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    const box = await composer();
    await waitFor(() => expect(qa("[data-reply]").length).toBeGreaterThan(0));
    // Several agents, nobody addressed yet: no chip.
    expect(q("[data-addressee]")).toBeNull();
    const reply = qa("[data-reply]").find((b) => b.dataset.reply === "claude")!;
    await act(async () => {
      fireEvent.click(reply);
    });
    await act(async () => {
      fireEvent.change(box, { target: { value: "@claude can you check the tests?", selectionStart: 32 } });
      fireEvent.keyDown(box, { key: "Enter" });
      await new Promise((r) => setTimeout(r, 30));
    });
    await waitFor(() => expect(q("[data-addressee-chip='claude']")).not.toBeNull());
    expect(q("[data-addressee]")?.getAttribute("aria-label")).toBe("Talking to Claude Code");
    expect(box.placeholder).toBe("Message Claude Code");
    // The next message names no one; the posted text does.
    await act(async () => {
      fireEvent.change(box, { target: { value: "and the lint", selectionStart: 12 } });
      fireEvent.keyDown(box, { key: "Enter" });
      await new Promise((r) => setTimeout(r, 30));
    });
    await waitFor(() => expect(qa("[data-entry]").some((e) => e.textContent?.includes("and the lint") && e.textContent.includes("@claude"))).toBe(true));
    // It is kept for this Space.
    expect(useAddressing.getState().bySpace[DEMO_THREAD_SPACE]).toMatchObject({ pinned: ["claude"], last: "claude", told: true });
    // The X removes it, and focus returns to the box.
    await act(async () => {
      fireEvent.click(q("[data-addressee-remove='claude']")!);
    });
    expect(q("[data-addressee]")).toBeNull();
    expect(document.activeElement).toBe(box);
    expect(useAddressing.getState().bySpace[DEMO_THREAD_SPACE]?.pinned).toEqual([]);
  });

  it("offers @everyone in the picker", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    const box = await composer();
    fireEvent.change(box, { target: { value: "@eve", selectionStart: 4 } });
    await waitFor(() => expect(q("[data-mention-option='everyone']")).not.toBeNull());
    expect(q("[data-mention-option='everyone']")?.getAttribute("aria-disabled")).toBeNull();
    fireEvent.keyDown(box, { key: "Enter" });
    expect(box.value).toBe("@everyone ");
  });
});

describe("an addressee that is not connected", () => {
  it("says so on the chip, and the send still goes", async () => {
    // Claude was addressed earlier in this Space and is not signed in.
    pinAgent(DEMO_THREAD_SPACE, "claude");
    await mountApp(path, { failsCreates: false, listsGhost: false, demo: { threadAuthFailed: true } });
    const box = await composer();
    // The chip uses the Connections wording for a sign-in that is missing.
    const chip = await waitFor(() => {
      const c = q("[data-addressee-chip='claude']");
      expect(c?.hasAttribute("data-addressee-problem")).toBe(true);
      return c!;
    });
    expect(chip.textContent).toBe("Claude Code needs to be connected");
    expect(chip.textContent).not.toContain("·");
    // The send still goes, with the name in front.
    await act(async () => {
      fireEvent.change(box, { target: { value: "try again", selectionStart: 9 } });
      fireEvent.keyDown(box, { key: "Enter" });
      await new Promise((r) => setTimeout(r, 30));
    });
    await waitFor(() => expect(qa("[data-entry]").some((e) => e.textContent?.includes("@claude try again"))).toBe(true));
  });
});

describe("the composer's chip", () => {
  const props = {
    placeholder: "Message room",
    candidates: [
      { handle: "claude", label: "Claude Code", kind: "agent" as const },
      { handle: "everyone", label: "Everyone", kind: "agent" as const, detail: "Only people who can give tasks can message everyone", disabled: true },
    ],
    handles: new Set(["claude"]),
    agentHarness: () => "claude-code",
    onSend: async () => {},
  };
  const agentHandles = new Set(["claude", "hermes"]);

  it("shows who it goes to, says when they can't take it, and goes with the X or Backspace", async () => {
    const onRemove = vi.fn();
    const { container } = render(
      <Composer {...props} addressee={{ handles: ["claude"], label: () => "Claude Code", problem: () => "Claude Code needs to be connected", onRemove, agentHandles }} />,
    );
    const chip = container.querySelector("[data-addressee-chip='claude']")!;
    expect(chip.hasAttribute("data-addressee-problem")).toBe(true);
    expect(chip.textContent).toBe("Claude Code needs to be connected");
    const x = container.querySelector<HTMLElement>("[data-addressee-remove='claude']")!;
    expect(x.getAttribute("aria-label")).toBe("Stop talking to Claude Code");
    fireEvent.click(x);
    expect(onRemove).toHaveBeenCalledWith("claude");
    onRemove.mockClear();
    fireEvent.keyDown(container.querySelector("[data-composer]")!, { key: "Backspace" });
    expect(onRemove).toHaveBeenCalledWith("claude");
  });

  it("steps aside while the text names an agent itself", () => {
    const { container } = render(<Composer {...props} addressee={{ handles: ["claude"], label: () => "Claude Code", problem: () => null, onRemove: () => {}, agentHandles }} />);
    const box = container.querySelector<HTMLTextAreaElement>("[data-composer]")!;
    expect(container.querySelector("[data-addressee]")).not.toBeNull();
    fireEvent.change(box, { target: { value: "@hermes hi", selectionStart: 10 } });
    expect(container.querySelector("[data-addressee]")).toBeNull();
  });

  it("lists a disabled @everyone with its reason and does not pick it", () => {
    const { container } = render(<Composer {...props} />);
    const box = container.querySelector<HTMLTextAreaElement>("[data-composer]")!;
    fireEvent.change(box, { target: { value: "@ev", selectionStart: 3 } });
    const opt = container.querySelector("[data-mention-option='everyone']")!;
    expect(opt.getAttribute("aria-disabled")).toBe("true");
    expect(opt.textContent).toContain("Only people who can give tasks can message everyone");
    fireEvent.keyDown(box, { key: "Enter" });
    expect(box.value).toBe("@ev");
  });

  it("fills the box and focuses it from its handle, once", () => {
    const ref = createRef<ComposerHandle>();
    const { container } = render(<Composer {...props} composerRef={ref} />);
    const box = container.querySelector<HTMLTextAreaElement>("[data-composer]")!;
    fireEvent.change(box, { target: { value: "look at this", selectionStart: 12 } });
    act(() => ref.current!.reply("claude"));
    expect(box.value).toBe("@claude look at this");
    expect(document.activeElement).toBe(box);
    act(() => ref.current!.reply("claude"));
    expect(box.value).toBe("@claude look at this");
  });
});
