// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Files dropped from Finder on a Space's tile on the Spaces page go into
// that Space, as a drop on its page's well does (real routes, wasm core,
// the demo host): the tile says where they go while they are over it, what
// the drop did shows in a toast, a stopped Space's tile refuses them, and an
// app dropped on a tile opens Teleport at that app.

import { cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const RUNNING = "relay:mac-mini/qa-windows";

function host(calls: [string, unknown][], answers: Record<string, (args: unknown) => unknown> = {}): TestHost {
  return {
    failsCreates: false,
    listsGhost: false,
    intercept: (op, args) => {
      calls.push([op, args]);
      return answers[op]?.(args);
    },
  };
}

const transfer = (names: string[]) => ({ types: ["Files"], files: names.map((n) => new File(["x"], n)), dropEffect: "none" });

const tileOf = (id: string) =>
  waitFor(() => {
    const link = document.querySelector<HTMLElement>(`[data-space-id="${id}"]`);
    if (!link) throw new Error(`no tile ${id}`);
    return link.parentElement!;
  });

describe.skipIf(!wasmBuilt)("dropping files on a Space's tile", () => {
  it("says where the files go while over the tile, sends the dropped files into that Space, and reports it", async () => {
    const calls: [string, unknown][] = [];
    await mountApp(
      "/spaces",
      host(calls, {
        "spaces.droppedFiles": () => ["/Users/ada/notes.txt", "/Users/ada/plan.pdf"],
        "spaces.sendFiles": () => [
          { name: "notes.txt", bytes: 10, remotePath: "/home/cua/Desktop/notes.txt" },
          { name: "plan.pdf", bytes: 20, remotePath: "/home/cua/Desktop/plan.pdf" },
        ],
      }),
    );
    const tile = await tileOf(RUNNING);
    await waitFor(() => expect(tile.querySelector('[data-space-id]')?.getAttribute("data-state")).toBe("running"));
    const dt = transfer(["notes.txt", "plan.pdf"]);

    fireEvent.dragEnter(tile, { dataTransfer: dt });
    fireEvent.dragOver(tile, { dataTransfer: dt });
    expect(tile.getAttribute("data-tile-drop")).toBe("accept");
    expect(tile.textContent).toContain("Send to qa-windows");
    expect(dt.dropEffect).toBe("copy");

    fireEvent.drop(tile, { dataTransfer: dt });
    expect(tile.getAttribute("data-tile-drop")).toBeNull();
    await waitFor(() => expect(calls).toContainEqual(["spaces.sendFiles", { spaceId: RUNNING, paths: ["/Users/ada/notes.txt", "/Users/ada/plan.pdf"] }]));
    expect(calls).toContainEqual(["spaces.droppedFiles", { names: ["notes.txt", "plan.pdf"] }]);
    await waitFor(() => expect(tile.getAttribute("data-drop-status")).toBe("done"));
    await waitFor(() => expect(document.body.textContent).toContain("Sent to qa-windows"));
  });

  it("says why when the send fails", async () => {
    const calls: [string, unknown][] = [];
    await mountApp(
      "/spaces",
      host(calls, {
        "spaces.droppedFiles": () => ["/Users/ada/notes.txt"],
        "spaces.sendFiles": () => {
          throw new Error("the Space did not answer");
        },
      }),
    );
    const tile = await tileOf(RUNNING);
    await waitFor(() => expect(tile.querySelector('[data-space-id]')?.getAttribute("data-state")).toBe("running"));
    const dt = transfer(["notes.txt"]);
    fireEvent.dragOver(tile, { dataTransfer: dt });
    fireEvent.drop(tile, { dataTransfer: dt });
    await waitFor(() => expect(tile.getAttribute("data-drop-status")).toBe("failed"));
    await waitFor(() => expect(document.body.textContent).toContain("Couldn't send to qa-windows"));
    expect(document.body.textContent).toContain("the Space did not answer");
  });

  it("refuses files on a Space that isn't running", async () => {
    const calls: [string, unknown][] = [];
    await mountApp("/spaces", host(calls));
    const link = await waitFor(() => {
      const el = document.querySelector<HTMLElement>('[data-space-id][data-state="stopped"]');
      if (!el) throw new Error("no stopped tile");
      return el;
    });
    const tile = link.parentElement!;
    const dt = transfer(["notes.txt"]);
    fireEvent.dragOver(tile, { dataTransfer: dt });
    expect(tile.getAttribute("data-tile-drop")).toBe("refuse");
    expect(dt.dropEffect).toBe("none");
    expect(tile.textContent).toContain("isn't running");
    fireEvent.drop(tile, { dataTransfer: dt });
    await new Promise((r) => setTimeout(r, 50));
    expect(calls.some(([op]) => op === "spaces.droppedFiles" || op === "spaces.sendFiles")).toBe(false);
  });

  it("opens Teleport at an app dropped on a tile", async () => {
    const calls: [string, unknown][] = [];
    await mountApp("/spaces", host(calls, { "spaces.droppedFiles": () => ["/Applications/Google Chrome.app"] }));
    const tile = await tileOf(RUNNING);
    await waitFor(() => expect(tile.querySelector('[data-space-id]')?.getAttribute("data-state")).toBe("running"));
    const dt = transfer(["Google Chrome.app"]);
    fireEvent.dragOver(tile, { dataTransfer: dt });
    fireEvent.drop(tile, { dataTransfer: dt });
    await waitFor(() => expect(calls.some(([op]) => op === "teleport.entryForPath")).toBe(true));
    expect(calls.some(([op]) => op === "spaces.sendFiles")).toBe(false);
  });
});
