// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// New Space's arrow: the second way to add, a machine by its address. It
// opens the address form (no image picker), and Run on no longer lists it.

import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetNewSpaceSession } from "@/bridge/new-space";
import { mountApp, stubBrowserApis } from "./app-harness";

beforeEach(resetNewSpaceSession);
afterEach(cleanup);
stubBrowserApis();

describe.skipIf(!wasmBuilt)("New Space split button", () => {
  it("opens the address form from the arrow's menu, and the wizard from the button", { timeout: 20_000 }, async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    fireEvent.click((await screen.findAllByRole("button", { name: "More ways to add" }))[0]!);
    const item = await screen.findByRole("menuitem", { name: "Add a machine by its address…" });
    fireEvent.click(item);
    const dialog = await waitFor(() => {
      const d = document.querySelector<HTMLElement>("[data-new-space]");
      expect(d).not.toBeNull();
      return d!;
    });
    await waitFor(() => expect(dialog.getAttribute("data-mode")).toBe("address"));
    expect(dialog.querySelector("[data-image-input]")).toBeNull();
    expect(dialog.querySelector("[data-run-on]")).toBeNull();
  });

  it("Run on has no address entry", { timeout: 20_000 }, async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    fireEvent.click((await screen.findAllByRole("button", { name: /^New Space$/ }))[0]!);
    const dialog = await waitFor(() => {
      const d = document.querySelector<HTMLElement>("[data-new-space]");
      expect(d).not.toBeNull();
      return d!;
    });
    fireEvent.click(await waitFor(() => dialog.querySelector<HTMLElement>("[data-run-on]")!));
    const values = await waitFor(() => {
      const v = [...document.querySelectorAll("[data-run-on-option]")].map((el) => el.getAttribute("data-value"));
      expect(v.length).toBeGreaterThan(0);
      return v;
    });
    expect(values.some((v) => v?.includes("address"))).toBe(false);
  });
});
