// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Notifications (CUA-1636): a line opens in place to its whole title and
// body, and closes again; opening it marks that one entry read; one about a
// Space offers Open Space, which goes to that Space's page. Mouse and
// keyboard alike.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { NotificationInput } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const LONG = `The nightly build passed on all three Spaces. ${"Every suite ran green, the flaky stream test included, and the release notes are drafted. ".repeat(4)}\nSecond paragraph: two follow-ups are in your Volume.`;

async function mount(spaceId: string | null) {
  const now = Date.now();
  let feed: NotificationInput[] = [
    { id: "n-long", atMs: now - 60_000, agent: "hermes", kind: "message", title: "hermes", body: LONG, read: false, space: spaceId },
    { id: "n-old", atMs: now - 3_600_000, agent: "ada", kind: "turn_ended", title: "ada", body: "Done.", read: true },
  ];
  const acks: string[][] = [];
  const app = await mountApp("/notifications", {
    failsCreates: false,
    listsGhost: false,
    intercept: (op, args) => {
      if (op === "notifications.list") return feed.map((n) => ({ ...n }));
      if (op === "notifications.markRead") {
        const { ids } = args as { ids: string[] };
        acks.push(ids);
        feed = feed.map((n) => (ids.includes(n.id) ? { ...n, read: true } : n));
        return null;
      }
      return undefined;
    },
  });
  await waitFor(() => expect(row("n-long")).not.toBeNull(), { timeout: 8000 });
  return { ...app, acks };
}

const row = (id: string) => document.querySelector<HTMLElement>(`[data-notification="${id}"]`);
const toggle = (id: string) => row(id)!.querySelector<HTMLButtonElement>("[data-notification-toggle]")!;
const text = (id: string) => row(id)!.querySelector<HTMLElement>("[data-notification-text]")!;

describe.skipIf(!wasmBuilt)("Notifications page", () => {
  it("opens a cut-off line in place to the whole text, marks it read, and closes it again", async () => {
    const { acks } = await mount(null);
    expect(row("n-long")!.dataset.unread).toBe("true");
    expect(toggle("n-long").getAttribute("aria-expanded")).toBe("false");
    expect(text("n-long").className).toContain("truncate");
    // The cut-off tooltip is gone: opening the line is how to read it.
    expect(text("n-long").hasAttribute("title")).toBe(false);
    expect(row("n-long")!.querySelector("[data-notification-detail]")).toBeNull();

    await act(async () => fireEvent.click(toggle("n-long")));
    expect(toggle("n-long").getAttribute("aria-expanded")).toBe("true");
    const detail = row("n-long")!.querySelector<HTMLElement>("[data-notification-detail]")!;
    expect(toggle("n-long").getAttribute("aria-controls")).toBe(detail.id);
    expect(text("n-long").className).not.toContain("truncate");
    expect(text("n-long").textContent).toBe("hermes");
    expect(detail.querySelector("[data-notification-body]")!.textContent).toBe(LONG.trim());
    expect(detail.querySelector("[data-notification-open-space]")).toBeNull();

    await waitFor(() => expect(acks).toEqual([["n-long"]]));
    await waitFor(() => expect(row("n-long")!.dataset.unread).toBe("false"));
    expect(document.querySelector("[data-mark-all]")).toBeNull();

    await act(async () => fireEvent.click(toggle("n-long")));
    expect(toggle("n-long").getAttribute("aria-expanded")).toBe("false");
    expect(row("n-long")!.querySelector("[data-notification-detail]")).toBeNull();
    expect(text("n-long").className).toContain("truncate");
  });

  it("does not ack an entry that is already read", async () => {
    const { acks } = await mount(null);
    await act(async () => fireEvent.click(toggle("n-old")));
    expect(row("n-old")!.querySelector("[data-notification-body]")!.textContent).toBe("Done.");
    await new Promise((r) => setTimeout(r, 50));
    expect(acks).toEqual([]);
  });

  it("is a focusable native button, so Enter and Space open and close it", async () => {
    await mount(null);
    const button = toggle("n-long");
    // A <button type="button"> turns Enter and Space into its click in every
    // browser (jsdom has no such activation, so the click stands in for it).
    expect(button.tagName).toBe("BUTTON");
    expect(button.type).toBe("button");
    expect(button.tabIndex).toBe(0);
    button.focus();
    expect(document.activeElement).toBe(button);
    await act(async () => button.click());
    expect(button.getAttribute("aria-expanded")).toBe("true");
    expect(document.activeElement).toBe(button);
    await act(async () => button.click());
    expect(button.getAttribute("aria-expanded")).toBe("false");
  });

  it("offers Open Space for a note about a Space, and it goes to that Space's page", async () => {
    const { demo } = await mount(null);
    const spaces = (await demo.call("spaces.list" as never, {} as never)) as { id: string }[];
    const spaceId = spaces[0]!.id;
    cleanup();
    const app = await mount(spaceId);
    await act(async () => fireEvent.click(toggle("n-long")));
    const open = row("n-long")!.querySelector<HTMLElement>("[data-notification-open-space]")!;
    expect(open.textContent).toBe("Open Space");
    expect(row("n-old")!.querySelector("[data-notification-open-space]")).toBeNull();
    await act(async () => fireEvent.click(open));
    await waitFor(() => expect(app.router.state.location.pathname).toBe(`/spaces/${encodeURIComponent(spaceId)}`));
  });
});
