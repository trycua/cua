// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Share and Add an agent lead to each other on the redesigned Space page:
// Share's footer has "Add an agent" left of Done, and Add an agent has
// "Share with a person". Each replaces the other (one dialog at a time,
// focus in the one that opened, Escape closes it). With the redesign off
// the Share sheet stays as it was.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { DEMO_THREAD_SPACE } from "@/bridge/adapters/demo/thread";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
});

const HOST = { failsCreates: false, listsGhost: false };
const path = `/spaces/${encodeURIComponent(DEMO_THREAD_SPACE)}`;
const q = (sel: string) => document.querySelector<HTMLElement>(sel);
const dialogs = () => document.querySelectorAll("[role=dialog]").length;
const flag = (on: boolean) => vi.stubEnv("VITE_CUA_FLAG_SPACES_REDESIGN", on ? "1" : "0");

async function openShare() {
  const share = await screen.findAllByRole("button", { name: "Share" });
  await act(async () => {
    fireEvent.click(share[0]!);
  });
  await waitFor(() => expect(q("[data-share-sheet]")).not.toBeNull());
}

describe.skipIf(!wasmBuilt)("Share and Add an agent", () => {
  it("redesign: Share opens Add an agent in its place, and Add an agent opens Share back", async () => {
    flag(true);
    await mountApp(path, HOST);
    await waitFor(() => expect(q("[data-composer]")).not.toBeNull());
    await openShare();
    const add = await waitFor(() => {
      const b = q("[data-share-add-agent]");
      expect(b).not.toBeNull();
      return b!;
    });
    expect(add.textContent).toBe("Add an agent");
    // Left of Done, in the footer.
    expect(add.nextElementSibling?.textContent).toBe("Done");
    await act(async () => {
      fireEvent.click(add);
    });
    await waitFor(() => expect(q("[data-add-agent]")).not.toBeNull());
    await waitFor(() => expect(q("[data-share-sheet]")).toBeNull());
    expect(dialogs()).toBe(1);
    expect(document.body.textContent).toContain("Add an agent to");

    await act(async () => {
      fireEvent.click(q("[data-add-row=share]")!);
    });
    await waitFor(() => expect(q("[data-share-sheet]")).not.toBeNull());
    await waitFor(() => expect(q("[data-add-agent]")).toBeNull());
    expect(dialogs()).toBe(1);
  });

  it("redesign off: the Share sheet has no Add an agent", async () => {
    flag(false);
    await mountApp(path, HOST);
    await waitFor(() => expect(document.body.textContent).toContain("ubuntu-build"));
    await openShare();
    expect(q("[data-share-add-agent]")).toBeNull();
    expect(screen.getByRole("button", { name: "Done" })).toBeTruthy();
  });
});
