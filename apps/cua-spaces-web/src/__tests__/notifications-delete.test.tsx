// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Notifications: each line has a delete control (reachable by keyboard, shown
// on hover and focus), Delete all asks first and names the count, and the
// sidebar's unread badge follows at once. Deleted entries stay deleted: the
// next poll of the host's feed does not bring them back.

import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { NotificationInput } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

async function mount(count = 3) {
  const now = Date.now();
  let feed: NotificationInput[] = Array.from({ length: count }, (_, i) => ({
    id: `n${i}`,
    atMs: now - (i + 1) * 60_000,
    agent: "ada",
    kind: "message",
    title: `ada ${i}`,
    body: `Body ${i}`,
    read: i === count - 1,
  }));
  const deletes: string[][] = [];
  let deleteAlls = 0;
  const app = await mountApp("/notifications", {
    failsCreates: false,
    listsGhost: false,
    intercept: (op, args) => {
      if (op === "notifications.list") return feed.map((n) => ({ ...n }));
      if (op === "notifications.delete") {
        const { ids } = args as { ids: string[] };
        deletes.push(ids);
        feed = feed.filter((n) => !ids.includes(n.id));
        return null;
      }
      if (op === "notifications.deleteAll") {
        deleteAlls += 1;
        feed = [];
        return null;
      }
      return undefined;
    },
  });
  await waitFor(() => expect(row("n0")).not.toBeNull(), { timeout: 8000 });
  return { ...app, deletes, deleteAlls: () => deleteAlls, feed: () => feed };
}

const row = (id: string) => document.querySelector<HTMLElement>(`[data-notification="${id}"]`);
const del = (id: string) => within(row(id)!).getByRole("button", { name: "Delete notification" });
const badge = () => document.querySelector<HTMLElement>("[data-unread-badge]");

describe.skipIf(!wasmBuilt)("Notifications delete", () => {
  it("has a delete control per line: a focusable button, shown on hover and on keyboard focus", async () => {
    await mount();
    const button = del("n0");
    expect(button.tagName).toBe("BUTTON");
    expect(button.tabIndex).toBe(0);
    // Quiet until the line is hovered or something in it has focus.
    expect(button.className).toContain("opacity-0");
    expect(button.className).toContain("group-hover/row:opacity-100");
    expect(button.className).toContain("group-focus-within/row:opacity-100");
    expect(button.className).toContain("focus-visible:opacity-100");
    // It is not inside the line's own button, so opening a line never deletes it.
    expect(button.closest("[data-notification-toggle]")).toBeNull();
    button.focus();
    expect(document.activeElement).toBe(button);
  });

  it("deletes one line at once, updates the unread badge, and keeps the others", async () => {
    const { deletes } = await mount();
    expect(badge()!.textContent).toBe("2");
    await act(async () => fireEvent.click(del("n0")));
    await waitFor(() => expect(row("n0")).toBeNull());
    expect(deletes).toEqual([["n0"]]);
    expect(row("n1")).not.toBeNull();
    expect(row("n2")).not.toBeNull();
    await waitFor(() => expect(badge()!.textContent).toBe("1"));
    // The next poll agrees: it stays gone.
    await new Promise((r) => setTimeout(r, 50));
    expect(row("n0")).toBeNull();
  });

  it("deleting a read line leaves the unread badge alone; deleting the last unread removes it", async () => {
    await mount();
    await act(async () => fireEvent.click(del("n2")));
    await waitFor(() => expect(row("n2")).toBeNull());
    expect(badge()!.textContent).toBe("2");
    await act(async () => fireEvent.click(del("n0")));
    await act(async () => fireEvent.click(del("n1")));
    await waitFor(() => expect(badge()).toBeNull());
  });

  it("asks before deleting all, naming the count, and Cancel deletes nothing", async () => {
    const app = await mount(27);
    await act(async () => fireEvent.click(document.querySelector("[data-delete-all]")!));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Delete all 27 notifications?")).toBeTruthy();
    await act(async () => fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" })));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(app.deleteAlls()).toBe(0);
    expect(document.querySelectorAll("[data-notification]").length).toBe(27);
  });

  it("deletes all after the confirmation and shows the empty state and no badge", async () => {
    const app = await mount(27);
    await act(async () => fireEvent.click(document.querySelector("[data-delete-all]")!));
    const dialog = await screen.findByRole("alertdialog");
    await act(async () => fireEvent.click(within(dialog).getByRole("button", { name: "Delete all" })));
    await waitFor(() => expect(document.querySelectorAll("[data-notification]").length).toBe(0));
    expect(app.deleteAlls()).toBe(1);
    expect(app.feed()).toEqual([]);
    expect(screen.getByText("No notifications.")).toBeTruthy();
    expect(badge()).toBeNull();
    // Nothing left to mark or delete.
    expect(document.querySelector("[data-mark-all]")).toBeNull();
    expect(document.querySelector("[data-delete-all]")).toBeNull();
  });

  it("stays fast with hundreds of lines", async () => {
    await mount(500);
    expect(document.querySelectorAll("[data-notification]").length).toBe(500);
    const started = performance.now();
    await act(async () => fireEvent.click(del("n250")));
    await waitFor(() => expect(row("n250")).toBeNull());
    expect(performance.now() - started).toBeLessThan(2000);
  });
});
