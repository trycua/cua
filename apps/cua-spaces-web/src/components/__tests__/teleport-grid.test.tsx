// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { PickerTile, TeleportSession, TeleportStore } from "@/bridge";
import { TeleportGrid } from "@/components/teleport/grid";
import TeleportDialog from "@/components/teleport/teleport-dialog";

const hooks = vi.hoisted(() => ({ teleport: null as unknown }));
vi.mock("@/bridge", async (actual) => ({ ...(await actual<typeof import("@/bridge")>()), useTeleport: () => hooks.teleport }));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

/** An IntersectionObserver whose targets are in view only once `show` is called for them. */
function stubObserver() {
  const observed = new Map<Element, (entries: { isIntersecting: boolean }[]) => void>();
  vi.stubGlobal(
    "IntersectionObserver",
    class {
      constructor(private readonly cb: (entries: { isIntersecting: boolean }[]) => void) {}
      observe(el: Element) {
        observed.set(el, this.cb);
      }
      disconnect() {}
    },
  );
  return (el: Element) => act(() => observed.get(el)?.([{ isIntersecting: true }]));
}

const tile = (id: string, over: Partial<PickerTile> = {}): PickerTile => ({
  id,
  title: id,
  help: "App and signed-in state",
  disabled: false,
  selected: false,
  icon: { kind: "host", path: `/Applications/${id}.app` },
  thumbnail: { kind: "none" },
  ...over,
});

describe("teleport grid", () => {
  it("says why an app cannot move, and asks for the icons of the tiles it draws", () => {
    const why = "no Linux build in the install manifest and no teleport provider for its state";
    const session = {
      grid: {
        sections: [
          { title: "Apps", tiles: [tile("Slack")] },
          { title: "Not available", tiles: [tile("1Password", { disabled: true, help: why })] },
        ],
        emptyText: null,
      },
    } as unknown as TeleportSession;
    const teleport = { icon: vi.fn(), thumbnail: vi.fn(), select: vi.fn(), activate: vi.fn(), step: vi.fn() } as unknown as TeleportStore;
    const images = new Map([["host:/Applications/Slack.app", "data:image/png;base64,AAAA"]]);
    const { container } = render(<TeleportGrid session={session} teleport={teleport} images={images} />);
    expect(screen.getByText(why).hasAttribute("data-tile-reason")).toBe(true);
    // An app that can move shows no reason line, only its tooltip.
    expect(container.querySelectorAll("[data-tile-reason]")).toHaveLength(1);
    expect(teleport.icon).toHaveBeenCalledTimes(2);
    expect(container.querySelector('[data-tile="Slack"] img')?.getAttribute("src")).toBe("data:image/png;base64,AAAA");
  });

  it("keeps tiles out of view out of the accessibility tree, so the dialog's buttons stay reachable", async () => {
    const show = stubObserver();
    const why = "no Linux build in the install manifest and no teleport provider for its state";
    const unavailable = Array.from({ length: 400 }, (_, i) => tile(`App ${i}`, { disabled: true, help: why }));
    const session = {
      spaceName: "eprecheck1",
      state: { step: "pick", entry: null },
      frame: {},
      tab: "apps",
      tabs: [{ tab: "apps", label: "Apps" }],
      query: "",
      primary: { label: "Continue", enabled: true },
      grid: { sections: [{ title: "Apps", tiles: [tile("Slack", { selected: true }), tile("Notion")] }, { title: "Not available", tiles: unavailable }], emptyText: null },
    } as unknown as TeleportSession;
    const teleport = { icon: vi.fn(), thumbnail: vi.fn(), select: vi.fn(), activate: vi.fn(), step: vi.fn(), close: vi.fn(), setTab: vi.fn(), search: vi.fn(), send: vi.fn() } as unknown as TeleportStore;
    hooks.teleport = { session, images: new Map(), teleport };
    const { container } = render(<TeleportDialog />);
    // The footer, by role and name.
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Continue" })).toBeTruthy();
    // Nothing in view yet: only the selected tile is exposed; all 402 are drawn.
    expect(screen.getAllByRole("option").map((o) => o.getAttribute("data-tile"))).toEqual(["Slack"]);
    expect(container.ownerDocument.querySelectorAll("[data-tile]")).toHaveLength(402);
    // Tiles that come into view join it.
    show(container.ownerDocument.querySelector('[data-tile="Notion"]')!);
    show(container.ownerDocument.querySelector('[data-tile="App 0"]')!);
    expect(screen.getAllByRole("option").map((o) => o.getAttribute("data-tile"))).toEqual(["Slack", "Notion", "App 0"]);
    expect(screen.getByRole("option", { name: /App 0/ }).getAttribute("aria-disabled")).toBe("true");
  });
});
