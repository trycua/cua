// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Space's page as a call (CUA-2155): the desktop on top, the thread below
// with tasks, replies and an approval, the members on the right. A daemon
// without the thread keeps the classic page exactly as it was.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { DEMO_THREAD_SPACE } from "@/bridge/adapters/demo/thread";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
const path = `/spaces/${encodeURIComponent(DEMO_THREAD_SPACE)}`;

/** Opens an agent's card from its row in In this space, with Details open. */
async function openRailCard(handle: string) {
  const row = await waitFor(() => {
    const el = q(`[data-profile-trigger='${handle}'] [data-member='agent:${handle}']`)?.closest<HTMLElement>("[data-profile-trigger]");
    expect(el).toBeTruthy();
    return el!;
  });
  await act(async () => {
    fireEvent.click(row);
  });
  const details = await waitFor(() => {
    const el = q(`[data-profile-card='agent'][data-profile='${handle}'] [data-card-details]`);
    expect(el).not.toBeNull();
    return el!;
  });
  await act(async () => {
    fireEvent.click(details);
  });
}

describe("a Space's thread", () => {
  it("draws the call layout: tasks with their state, a reply under its task, notes, an approval, the divider and the members", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(qa("[data-task]").length).toBeGreaterThan(2), { timeout: 4000 });
    expect(q("[data-call-layout]")).not.toBeNull();
    expect(qa("[data-task-chip]").map((c) => c.dataset.taskChip)).toEqual(expect.arrayContaining(["done", "working", "queued"]));
    // Claude Code's reply sits inside the task it answers, with its files.
    const done = q('[data-task-state="done"]')!;
    expect(done.querySelector("[data-task-replies] [data-attachment='report.html']")).not.toBeNull();
    expect(qa("[data-thread-note]").map((n) => n.textContent)).toContain("Hermes woke up and joined");
    expect(qa("[data-unread]")).toHaveLength(1);
    await waitFor(() => expect(q("[data-member='agent:claude']")?.textContent).toContain("Claude Code"));
    // A member's @name draws as a chip once the members are known.
    await waitFor(() => expect(q("[data-mention='claude']")?.textContent).toBe("@claude"));
    expect(q("[data-member='agent:claude']")?.textContent).toContain("bumping the retry timeout");
    expect(q("[data-member='agent:hermes']")?.dataset.memberStatus).toBe("asleep");
    expect(qa("[data-members] [data-member]").map((m) => m.textContent)).toEqual(expect.arrayContaining([expect.stringContaining("Grace"), expect.stringContaining("Linus")]));
    // P1 draws no presence: there is nothing to say who is here yet.
    expect(q("[data-presence]")).toBeNull();
  });

  it("gives an agent a task with @: the picker, the pick, the post, and the agent's answer", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    const box = await waitFor(() => {
      const el = q("[data-composer]") as HTMLTextAreaElement | null;
      expect(el?.disabled).toBe(false);
      return el!;
    });
    fireEvent.change(box, { target: { value: "@", selectionStart: 1 } });
    await waitFor(() => expect(q("[data-mention-picker]")).not.toBeNull());
    expect(qa("[data-mention-option]").map((o) => o.dataset.mentionOption)).toEqual(["claude", "hermes", "cursor", "codex", "openclaw", "grace", "linus"]);
    fireEvent.keyDown(box, { key: "ArrowDown" });
    fireEvent.keyDown(box, { key: "Enter" });
    expect(box.value).toBe("@hermes ");
    fireEvent.change(box, { target: { value: "@hermes draft the release note", selectionStart: 30 } });
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
      await new Promise((r) => setTimeout(r, 20));
    });
    const mine = await waitFor(() => {
      const t = qa("[data-task]").find((el) => el.textContent?.includes("draft the release note"));
      expect(t).toBeDefined();
      return t!;
    });
    expect(box.value).toBe("");
    // The demo's Hermes picks it up and answers under it.
    await waitFor(() => expect(mine.dataset.taskState).toBe("done"), { timeout: 4000 });
    expect(mine.querySelector("[data-task-replies]")?.textContent).toContain("Done.");
  });

  it("approves what an agent asked", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    const card = await waitFor(() => {
      const c = q('[data-approval-state="open"]');
      expect(c).not.toBeNull();
      return c!;
    });
    await act(async () => {
      fireEvent.click([...card.querySelectorAll("button")].find((b) => b.textContent === "Approve")!);
      await new Promise((r) => setTimeout(r, 20));
    });
    await waitFor(() => expect(card.dataset.approvalState).toBe("approved"));
    expect(card.querySelector("[data-approval-answer]")?.textContent).toContain("Approved by Ada");
  });

  it("lists the desktop and the first four windows on the right, pops one out from its hover button, and opens the rest from Show all", async () => {
    const { demo } = await mountApp(path, { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(qa("[data-stream-window]")).toHaveLength(4), { timeout: 4000 });
    expect(q("[data-stream-window='w-terminal']")?.textContent).toContain("Terminal");
    // The desktop is the default view: first, and the current one.
    expect(q("[data-stream-desktop]")?.getAttribute("aria-current")).toBe("true");
    await act(async () => {
      fireEvent.click(q("[data-stream-popout='w-firefox']")!);
      await new Promise((r) => setTimeout(r, 50));
    });
    await waitFor(async () => expect((await demo.call("spaces.windows", { spaceId: DEMO_THREAD_SPACE })).open).toContain("w-firefox"));
    // Popping out does not change what the stage shows.
    expect(q("[data-stream-desktop]")?.getAttribute("aria-current")).toBe("true");
    fireEvent.click(q("[data-stream-show-all]")!);
    await waitFor(() => expect(q("[data-call-more-sheet] [data-section='stream']")).not.toBeNull());
  });

  it("shows a window on the stage when it is picked, and a post made then says where it was sent from", async () => {
    const { demo } = await mountApp(path, { failsCreates: false, listsGhost: false });
    const firefox = await waitFor(() => {
      const el = q("[data-stream-window='w-firefox']");
      expect(el).not.toBeNull();
      return el!;
    }, { timeout: 4000 });
    await act(async () => {
      fireEvent.click(firefox);
      await new Promise((r) => setTimeout(r, 20));
    });
    expect(firefox.getAttribute("aria-current")).toBe("true");
    expect(q("[data-stream-desktop]")?.getAttribute("aria-current")).toBeNull();
    // The main view now shows Firefox, not the desktop.
    expect(q("[data-stream-surface]")?.textContent).toContain("Firefox opens in its own window.");
    // Picking is not popping out.
    expect((await demo.call("spaces.windows", { spaceId: DEMO_THREAD_SPACE })).open).not.toContain("w-firefox");
    const box = await waitFor(() => {
      const el = q("[data-composer]") as HTMLTextAreaElement | null;
      expect(el?.disabled).toBe(false);
      return el!;
    });
    fireEvent.change(box, { target: { value: "is this the right PR?", selectionStart: 21 } });
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
      await new Promise((r) => setTimeout(r, 20));
    });
    const line = await waitFor(() => {
      const el = qa("[data-entry]").find((e) => e.textContent?.includes("is this the right PR?"))?.querySelector("[data-entry-view]");
      expect(el).toBeTruthy();
      return el!;
    });
    expect(line.textContent).toBe("Sent from Firefox");
    expect(line.getAttribute("data-entry-view")).toBe("window");
    // The entry carries the structured view the daemon stores.
    const page = await demo.call("thread.read", { spaceId: DEMO_THREAD_SPACE, limit: 200 });
    const posted = page.entries.find((e) => e.text === "is this the right PR?");
    expect(posted?.view).toEqual({ kind: "window", app: "Firefox", title: "Pull requests · trycua/cua — Mozilla Firefox", windowId: "w-firefox" });

    // Back to the desktop: the next post carries no view and no line.
    await act(async () => {
      fireEvent.click(q("[data-stream-desktop]")!);
      await new Promise((r) => setTimeout(r, 20));
    });
    fireEvent.change(box, { target: { value: "and the build?", selectionStart: 14 } });
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
      await new Promise((r) => setTimeout(r, 20));
    });
    await waitFor(() => expect(qa("[data-entry]").some((e) => e.textContent?.includes("and the build?"))).toBe(true));
    const plain = qa("[data-entry]").find((e) => e.textContent?.includes("and the build?"))!;
    expect(plain.querySelector("[data-entry-view]")).toBeNull();
  });

  it("a guest reads: the composer says why it is off, and no Approve or Deny (CUA-2156)", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false, demo: { threadRole: "guest" } });
    const box = await waitFor(() => {
      const el = q("[data-composer-off]") as HTMLTextAreaElement | null;
      expect(el).not.toBeNull();
      return el!;
    });
    expect(box.disabled).toBe(true);
    expect(box.placeholder).toBe("View only. You can read this thread but not post.");
    expect(q("[data-send]")).toBeNull();
    const card = await waitFor(() => {
      const c = q('[data-approval-state="open"] [data-approval-waiting]');
      expect(c).not.toBeNull();
      return c!;
    });
    // Grace's Claude Code: its approvals go to Grace (CUA-2262).
    expect(card.textContent).toBe("Waiting for Grace");
    expect(qa('[data-approval-state="open"] button:not([data-author-name]):not([data-profile-trigger])')).toHaveLength(0);
  });

  it("says whose keys a task runs on, and shows rights read-only to a member", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false, demo: { threadRole: "member" } });
    await waitFor(() => expect(qa("[data-entry-footnote]").length).toBeGreaterThan(0), { timeout: 4000 });
    // Grace owns the Space here: Linus's task runs on her keys, hers on nobody else's.
    expect(qa("[data-entry-footnote]").map((f) => f.textContent)).toContain("Runs on Grace's keys");
    // Its row in In this space opens its card; who may task, stop and approve it is under Details, read-only.
    await openRailCard("hermes");
    expect(qa("[data-card-details-open] dt").map((d) => `${d.textContent}: ${d.nextElementSibling?.textContent}`)).toEqual(
      expect.arrayContaining(["Can give it tasks: Owner", "Can stop it: Admins", "Can approve its steps: Admins"]),
    );
    expect(q("[data-agent-right]")).toBeNull();
  });

  it("lets the owner change who can task an agent", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    await openRailCard("hermes");
    await waitFor(() => expect(q("[data-agent-rights='hermes'] [data-agent-right='task'] button")).not.toBeNull());
    expect(q("[data-agent-rights='hermes'] [data-agent-right='task']")?.textContent).toContain("Owner");
    expect(qa("[data-agent-rights='hermes'] [data-agent-right] button")).toHaveLength(3);
  });

  it("keeps the classic page when the daemon has no thread", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false, noThread: true });
    await waitFor(() => expect(q('[data-section="agents"], [data-fact="Status"]')).not.toBeNull(), { timeout: 4000 });
    expect(q("[data-call-layout]")).toBeNull();
    expect(q("[data-thread]")).toBeNull();
    expect(q("[data-composer]")).toBeNull();
  });
});

describe("the Stream rail's Desktop row", () => {
  it("shows the Space's own OS mark", async () => {
    const { osOfIcon } = await import("@/components/space-thread/members-panel");
    expect(osOfIcon("os-macos")).toBe("macos");
    expect(osOfIcon("os-windows")).toBe("windows");
    expect(osOfIcon("os-ubuntu")).toBe("linux");
  });
});
