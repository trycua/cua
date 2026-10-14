// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A notification's right-click menu (CUA-1639): Open or Close it in place,
// Mark as Read while unread, Open Space when it is about one, and Copy Text.
// The daemon can only mark entries read: no Mark as Unread or Dismiss.

import { cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { NotificationInput, WkContextMenuRequest } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { useContextMenuStore } from "@/components/context-menu";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(() => {
  cleanup();
  useContextMenuStore.setState({ open: null, confirming: null });
});
stubBrowserApis();

async function mount(answer: (m: WkContextMenuRequest) => string | null) {
  const now = Date.now();
  let feed: NotificationInput[] = [
    { id: "n-new", atMs: now - 60_000, agent: "hermes", kind: "message", title: "hermes", body: "The build is ready.", read: false, space: "local:design-review" },
    { id: "n-old", atMs: now - 3_600_000, agent: "ada", kind: "turn_ended", title: "ada", body: "Done.", read: true },
  ];
  const acks: string[][] = [];
  const shown: WkContextMenuRequest[] = [];
  const app = await mountApp("/notifications", {
    failsCreates: false,
    listsGhost: false,
    showContextMenu: async (m) => (shown.push(m), answer(m)),
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
  await waitFor(() => expect(row("n-new")).not.toBeNull(), { timeout: 8000 });
  return { ...app, acks, shown };
}

const row = (id: string) => document.querySelector<HTMLElement>(`[data-notification="${id}"]`);
const toggle = (id: string) => row(id)!.querySelector<HTMLButtonElement>("[data-notification-toggle]")!;
const items = (m: WkContextMenuRequest) => m.items.map((i) => (i.separator ? "-" : i.id));

describe.skipIf(!wasmBuilt)("a notification's menu", () => {
  it("offers Open, Mark as Read while unread, Open Space when it is about one, and Copy Text", async () => {
    const { shown } = await mount(() => null);
    expect(fireEvent.contextMenu(toggle("n-new"), { clientX: 10, clientY: 10, button: 2 })).toBe(false);
    await waitFor(() => expect(shown).toHaveLength(1));
    expect(items(shown[0]!)).toEqual(["notification.open", "notification.markRead", "notification.openSpace", "-", "notification.copy"]);
    expect(shown[0]!.items.find((i) => i.id === "notification.copy")?.copy).toBe("hermes\n\nThe build is ready.");
    fireEvent.contextMenu(toggle("n-old"), { clientX: 10, clientY: 10, button: 2 });
    await waitFor(() => expect(shown).toHaveLength(2));
    expect(items(shown[1]!)).toEqual(["notification.open", "-", "notification.copy"]);
  });

  it("Open expands it in place and marks it read; Close closes it", async () => {
    const { acks } = await mount((m) => (m.items.some((i) => i.id === "notification.open") ? "notification.open" : "notification.close"));
    fireEvent.contextMenu(toggle("n-new"), { clientX: 10, clientY: 10, button: 2 });
    await waitFor(() => expect(toggle("n-new").getAttribute("aria-expanded")).toBe("true"));
    await waitFor(() => expect(acks).toEqual([["n-new"]]));
    fireEvent.contextMenu(toggle("n-new"), { clientX: 10, clientY: 10, button: 2 });
    await waitFor(() => expect(toggle("n-new").getAttribute("aria-expanded")).toBe("false"));
  });

  it("Mark as Read marks just that one, without opening it", async () => {
    const { acks } = await mount(() => "notification.markRead");
    fireEvent.contextMenu(toggle("n-new"), { clientX: 10, clientY: 10, button: 2 });
    await waitFor(() => expect(acks).toEqual([["n-new"]]));
    await waitFor(() => expect(row("n-new")!.dataset.unread).toBe("false"));
    expect(toggle("n-new").getAttribute("aria-expanded")).toBe("false");
  });

  it("Open Space goes to its Space's page", async () => {
    const { router } = await mount(() => "notification.openSpace");
    fireEvent.contextMenu(toggle("n-new"), { clientX: 10, clientY: 10, button: 2 });
    await waitFor(() => expect(decodeURIComponent(router.state.location.pathname)).toBe("/spaces/local:design-review"));
  });
});
