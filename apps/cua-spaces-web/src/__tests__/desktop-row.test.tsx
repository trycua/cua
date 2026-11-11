// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// This Mac's desktop is This Mac's own row in the list of your computers,
// never counted as a Space, and Home's strip lists running Spaces first. Its
// page has no stream: it says whether the desktop is shared and holds the
// switch.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetHiddenDesktops } from "@/lib/hidden-desktops";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetHiddenDesktops();
});
afterEach(cleanup);

/** This Mac's roster row: reachable while its desktop is shared. */
const thisMac = (shared: boolean) => ({
  id: "this-mac",
  name: "This Mac",
  provider: "local",
  spacesdVersion: "",
  features: [],
  reachable: shared,
  os: "macos",
});

const withDesktop = (shared: boolean, only = false) => ({
  failsCreates: false,
  listsGhost: false,
  answer: (op: string, out: unknown) => {
    if (op === "host.status") return { ...(out as object), shareDesktop: shared };
    if (op === "machines.list")
      return (out as { current?: boolean; host?: object }[]).map((r) => (r.current && r.host ? { ...r, host: { ...r.host, shareDesktop: shared } } : r));
    return op === "spaces.list" ? (only ? [thisMac(shared)] : [...(out as unknown[]), thisMac(shared)]) : out;
  },
});

describe.skipIf(!wasmBuilt)("This Mac's desktop row", () => {
  it("never counts as a Space: the line under Computers stays the same with it listed", async () => {
    const summary = () => document.querySelector("main header p, header p")?.textContent ?? "";
    await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    const without = await waitFor(() => {
      expect(summary()).toMatch(/\d+ computers?/);
      return summary();
    });
    cleanup();
    await mountApp("/spaces", withDesktop(false));
    await waitFor(() => expect(document.querySelector("[data-spaces-tree]")).not.toBeNull());
    await waitFor(() => expect(summary()).toBe(without));
  });

  it("its page is a Space's page with the share prompt where the stream would be, and the chat beside it", async () => {
    const actions: string[] = [];
    await mountApp("/spaces/this-mac", {
      ...withDesktop(false),
      intercept: (op, args) => {
        if (op === "host.action") {
          actions.push((args as { action: string }).action);
          return null;
        }
        return undefined;
      },
    });
    const stage = await waitFor(() => {
      const el = document.querySelector<HTMLElement>("[data-this-mac-desktop]");
      expect(el).not.toBeNull();
      return el!;
    });
    // The same page as any Space: header, call layout, thread and composer.
    expect(document.querySelector("[data-call-layout]")).not.toBeNull();
    expect(document.querySelector("textarea")).not.toBeNull();
    expect(stage.getAttribute("data-this-mac-desktop")).toBe("off");
    expect(stage.textContent).toContain("You aren't sharing your desktop as a Space");
    expect(document.querySelector("[data-stream-surface], video, canvas")).toBeNull();
    expect(document.body.textContent).not.toContain("\u00b7");

    // Writing while not sharing lights the prompt and keeps the draft.
    const box = document.querySelector<HTMLTextAreaElement>("textarea")!;
    await act(async () => void fireEvent.change(box, { target: { value: "look at my screen" } }));
    await act(async () => void fireEvent.keyDown(box, { key: "Enter" }));
    await waitFor(() => expect(document.querySelector("[data-this-mac-desktop][data-nudge]")).not.toBeNull());
    expect(document.querySelector<HTMLTextAreaElement>("textarea")!.value).toBe("look at my screen");

    // The button turns sharing on.
    await act(async () => void fireEvent.click(document.querySelector("[data-desktop-share-button]")!));
    await waitFor(() => expect(actions).toContain("share-desktop"));
  });

  it("once the desktop is shared its chat works as in any Space: the message is posted to the thread", async () => {
    const posts: string[] = [];
    await mountApp("/spaces/this-mac", {
      ...withDesktop(true),
      intercept: (op, args) => {
        if (op === "thread.post") posts.push((args as { spaceId: string; text: string }).spaceId + ": " + (args as { text: string }).text);
        return undefined;
      },
    });
    const box = await waitFor(() => {
      const el = document.querySelector<HTMLTextAreaElement>("textarea");
      expect(el).not.toBeNull();
      return el!;
    });
    await act(async () => void fireEvent.change(box, { target: { value: "look at my screen" } }));
    await act(async () => void fireEvent.keyDown(box, { key: "Enter" }));
    await waitFor(() => expect(posts).toEqual(["this-mac: look at my screen"]));
    // It stays: the message is in the thread (not an error, not the share prompt).
    await waitFor(() => expect(document.body.textContent).toContain("look at my screen"));
    expect(document.querySelector("[data-this-mac-desktop][data-nudge]")).toBeNull();
    expect(document.body.textContent).not.toContain("Chat isn't available");
    expect(document.body.textContent).not.toContain("\u00b7");
  });
});

describe.skipIf(!wasmBuilt)("Home's Your Spaces", () => {
  it("keeps its tiles in place across refreshes while the host restamps the desktops", async () => {
    // The host stamps a desktop with the time it read the list: from one poll to the next This Mac's
    // and another machine's pass each other, and the tiles swapped places on every refresh.
    let poll = 0;
    const studio = { id: "relay:studio", name: "Studio", provider: "relay", spacesdVersion: "", features: [], reachable: true, os: "macos" };
    const at = (ms: number) => new Date(Date.UTC(2026, 10, 8) + ms).toISOString();
    const { hooks } = await mountApp("/home/ask", {
      failsCreates: false,
      listsGhost: false,
      answer: (op, out) => {
        if (op === "host.status") return { ...(out as object), shareDesktop: true };
        if (op !== "spaces.list") return out;
        poll += 1;
        const mac = { ...thisMac(true), addedAt: at(poll % 2 ? 2_000 : 1_000) };
        const other = { ...studio, addedAt: at(poll % 2 ? 1_000 : 2_000) };
        return poll % 2 ? [mac, other] : [other, mac];
      },
    });
    const tiles = () => [...document.querySelectorAll<HTMLElement>("[data-home-spaces] [data-space-id]")];
    const first = await waitFor(() => {
      expect(tiles().map((t) => t.getAttribute("data-space-id"))).toEqual(["this-mac", "relay:studio"]);
      return tiles();
    });
    for (let i = 0; i < 2; i++) {
      const before = poll;
      await act(() => hooks.spaces.refresh());
      expect(poll).toBeGreaterThan(before);
      // Same order, and the same DOM nodes: nothing remounted.
      expect(tiles()).toEqual(first);
      tiles().forEach((t, j) => expect(t).toBe(first[j]));
    }
  });

  it("lists running Spaces first, even when the list holds them last", async () => {
    await mountApp("/home/ask", {
      failsCreates: false,
      listsGhost: false,
      // The host lists them in the opposite order: the running ones come last.
      answer: (op, out) => (op === "spaces.list" ? [...(out as unknown[])].reverse() : out),
    });
    const states = await waitFor(() => {
      const tiles = [...document.querySelectorAll<HTMLElement>("[data-home-spaces] [data-space-id]")];
      expect(tiles.length).toBeGreaterThan(0);
      return tiles.map((t) => t.getAttribute("data-state"));
    });
    const firstOther = states.findIndex((s) => s !== "running");
    expect(states[0]).toBe("running");
    if (firstOther >= 0) expect(states.slice(firstOther).every((s) => s !== "running")).toBe(true);
    // The sidebar names the page for what it lists: your computers, with Machines part of it.
    expect(screen.getAllByText("Computers").length).toBeGreaterThan(0);
    expect(screen.queryByText("Machines")).toBeNull();
  });

  it("hidden from Home, it says how many are hidden with Show, and comes back from there or from its page", async () => {
    const { router } = await mountApp("/home/ask", withDesktop(true, true));
    const tile = () => document.querySelector('[data-home-spaces] [data-space-id="this-mac"]');
    await waitFor(() => expect(tile()).not.toBeNull(), { timeout: 15_000 });
    await act(
      async () =>
        void fireEvent.click(
          tile()!.querySelector<HTMLElement>("[data-tile-hide]") ?? document.querySelector<HTMLElement>("[data-home-spaces] [data-tile-hide]")!,
        ),
    );
    await waitFor(() => expect(tile()).toBeNull());
    // Never gone for good: "1 hidden · Show" stays under Home's computers.
    const line = await waitFor(() => {
      const el = document.querySelector<HTMLElement>("[data-home-hidden]");
      expect(el).not.toBeNull();
      return el!;
    });
    expect(line.textContent).toBe("1 hidden·Show");
    await act(async () => void fireEvent.click(document.querySelector<HTMLElement>("[data-home-show-hidden]")!));
    await waitFor(() => expect(tile()).not.toBeNull());
    expect(document.querySelector("[data-home-hidden]")).toBeNull();

    // Hidden again, its own page has the switch that shows it on Home.
    await act(async () => void fireEvent.click(document.querySelector<HTMLElement>("[data-home-spaces] [data-tile-hide]")!));
    await waitFor(() => expect(tile()).toBeNull());
    await act(async () => void router.navigate({ to: "/spaces/computer/$computerId", params: { computerId: "this-mac" } }));
    const row = await waitFor(
      () => {
        const el = document.querySelector<HTMLElement>("[data-machine-on-home]");
        expect(el).not.toBeNull();
        return el!;
      },
      { timeout: 15_000 },
    );
    expect(row.getAttribute("data-machine-on-home")).toBe("off");
    expect(row.textContent).toContain("Hidden from Home");
    await act(async () => void fireEvent.click(row.querySelector<HTMLElement>("[role=switch]")!));
    await waitFor(() => expect(document.querySelector("[data-machine-on-home]")!.getAttribute("data-machine-on-home")).toBe("on"));
  });
});
