// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Space's page draws the SwiftUI detail's sections after Stream: Agents
// (one line per run, the core's words) and Teleport (the drop well: Send
// file…, dropped files, an app dropped opens Teleport at it).

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const SPACE = "local:design-review";

function host(answers: Record<string, (out: unknown, args?: unknown) => unknown> = {}): TestHost {
  return { failsCreates: false, listsGhost: false, answer: (op, out) => (answers[op] ? answers[op]!(out) : out) };
}

/** A drop of files with these names on the well. */
function dropOn(well: Element, names: string[]) {
  const files = names.map((n) => new File(["x"], n));
  const dataTransfer = { types: ["Files"], files, dropEffect: "none" };
  fireEvent.dragOver(well, { dataTransfer });
  fireEvent.drop(well, { dataTransfer });
}

describe.skipIf(!wasmBuilt)("a Space's Agents and Teleport sections (real routes, wasm core)", () => {
  it("lists the Space's agent runs in the core's words", async () => {
    const { router } = await mountApp(`/spaces/${SPACE}`, host());
    await router.load();
    const section = await waitFor(() => {
      const s = document.querySelector('[data-section="agents"]');
      expect(s?.querySelector("[data-agent-run]")).not.toBeNull();
      return s!;
    });
    expect(section.querySelector("h2")?.textContent).toBe("Agents");
    const row = section.querySelector("[data-agent-run]")!;
    expect(row.textContent).toContain("·");
    expect(row.textContent).toMatch(/Running|Idle|Failed|Crashed|Unknown/);
  });

  it("sends picked and dropped files with the core's lines, and opens Teleport at a dropped app", async () => {
    const { router } = await mountApp(
      `/spaces/${SPACE}`,
      host({
        "spaces.chooseFiles": () => ["/Users/ada/notes.txt"],
        "spaces.droppedFiles": () => ["/Users/ada/a.txt", "/Users/ada/b.txt"],
      }),
    );
    await router.load();
    await screen.findByText("Drop a file or window");
    fireEvent.click(screen.getByRole("button", { name: "Send file…" }));
    await waitFor(() => expect(document.querySelector('[data-drop-status="done"]')?.textContent).toContain("notes.txt"));
    dropOn(document.querySelector(`[data-drop-well="${SPACE}"]`)!, ["a.txt", "b.txt"]);
    await waitFor(() => expect(document.querySelector('[data-drop-status="done"]')?.textContent).toMatch(/^2 files/));
  });

  it("opens Teleport at an app dropped on the well", async () => {
    const { router } = await mountApp(`/spaces/${SPACE}`, host({ "spaces.droppedFiles": () => ["/Applications/Google Chrome.app"] }));
    await router.load();
    const well = await waitFor(() => {
      const w = document.querySelector(`[data-drop-well="${SPACE}"]`);
      expect(w).not.toBeNull();
      return w!;
    });
    act(() => dropOn(well, ["Google Chrome.app"]));
    await waitFor(() => expect(document.querySelector("[data-teleport]")).not.toBeNull());
  });
});
