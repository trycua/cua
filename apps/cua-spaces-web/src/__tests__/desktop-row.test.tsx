// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// This Mac's desktop is a normal row of the Spaces list: "Not sharing" while its
// switch is off, "Sharing" while it is on, both with the "Desktop" tag. Hover
// shows Hide instead of power; "Show hidden" in the list's overflow brings it
// back. It never counts as a Space (the empty state, the count, "N running"),
// and Home's strip lists running Spaces first. Its page has no stream: it says
// whether the desktop is shared and holds the switch.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { setFlagOverride } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetHiddenDesktops } from "@/lib/hidden-desktops";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetHiddenDesktops();
  setFlagOverride("spaces_ask_first_home", true);
});
afterEach(cleanup);

/** This Mac's roster row: reachable while its desktop is shared. */
const thisMac = (shared: boolean) => ({ id: "this-mac", name: "This Mac", provider: "local", spacesdVersion: "", features: [], reachable: shared, os: "macos" });

const withDesktop = (shared: boolean, only = false) => ({
  failsCreates: false,
  listsGhost: false,
  answer: (op: string, out: unknown) => {
    if (op === "host.status") return { ...(out as object), shareDesktop: shared };
    if (op === "machines.list") return (out as { current?: boolean; host?: object }[]).map((r) => (r.current && r.host ? { ...r, host: { ...r.host, shareDesktop: shared } } : r));
    return op === "spaces.list" ? (only ? [thisMac(shared)] : [...(out as unknown[]), thisMac(shared)]) : out;
  },
});

const row = () => document.querySelector<HTMLElement>('[data-space-id="this-mac"]');
const tile = () => row()?.parentElement ?? null;

describe.skipIf(!wasmBuilt)("This Mac's desktop row", () => {
  it("reads Not sharing with a Desktop tag, Hide in place of power, and Show hidden brings it back", async () => {
    await mountApp("/spaces", withDesktop(false));
    await waitFor(() => expect(row()).not.toBeNull());
    expect(row()!.getAttribute("data-state")).toBe("stopped");
    expect(row()!.textContent).toContain("Not sharing");
    expect(row()!.querySelector("[data-desktop-tag]")?.textContent).toBe("Desktop");
    expect(tile()!.querySelector("[data-tile-power]")).toBeNull();
    expect(tile()!.querySelector("[data-select]")).toBeNull();

    await act(async () => void fireEvent.click(tile()!.querySelector("[data-tile-hide]")!));
    await waitFor(() => expect(row()).toBeNull());
    await act(async () => void fireEvent.pointerDown(document.querySelector("[data-toolbar-menu='overflow']")!, { button: 0 }));
    await act(async () => void fireEvent.click(document.querySelector("[data-toolbar-menu='overflow']")!));
    const show = await waitFor(() => {
      const el = document.querySelector<HTMLElement>("[data-show-hidden]");
      expect(el).not.toBeNull();
      return el!;
    });
    await act(async () => void fireEvent.click(show));
    await waitFor(() => expect(row()).not.toBeNull());
  });

  it("never counts as a Space: the count and the running number stay the same with it listed", async () => {
    const summary = () => document.querySelector("main header p, header p")?.textContent ?? "";
    await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    const without = await waitFor(() => {
      expect(summary()).toMatch(/\d+ Spaces? on [1-9]/);
      return summary();
    });
    cleanup();
    await mountApp("/spaces", withDesktop(false));
    await waitFor(() => expect(row()).not.toBeNull());
    await waitFor(() => expect(summary()).toBe(without));
  });

  it("with no other Space and not sharing, the Spaces page is the normal empty state", async () => {
    await mountApp("/spaces", withDesktop(false, true));
    await waitFor(() => expect(document.querySelector("[data-empty-home]")).not.toBeNull());
    expect(row()).toBeNull();
    expect(document.querySelector("[data-space-list], [data-tile-picture]")).toBeNull();
  });

  it("with no other Space and sharing, the list holds the desktop row, running", async () => {
    await mountApp("/spaces", withDesktop(true, true));
    await waitFor(() => expect(row()).not.toBeNull());
    expect(row()!.getAttribute("data-state")).toBe("running");
    expect(row()!.textContent).toContain("Sharing");
    expect(document.querySelector("[data-empty-home]")).toBeNull();
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
  it("lists running Spaces first, even when the list holds them last", async () => {
    await mountApp("/home", {
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
    // The sidebar names the page for what it lists.
    expect(screen.getAllByText("Machines").length).toBeGreaterThan(0);
    expect(screen.queryByText("Computers")).toBeNull();
  });
});
