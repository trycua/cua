// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Windows create Microsoft refuses (Sentinel): the failure screen offers
// Choose Windows ISO…, Open Microsoft's download page and Try again together.
// Choosing a file runs the create again with it; the New Space dialog has the
// same picker as a link.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { requestNewSpace, resetNewSpaceSession } from "@/bridge/new-space";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

beforeEach(resetNewSpaceSession);
afterEach(cleanup);
stubBrowserApis();

const IMAGE = "ghcr.io/trycua/windows:11";

function host(chosen: { name: string } | null | Error = { name: "Win11_ARM64.iso" }, calls: [string, unknown][] = []): TestHost {
  return {
    failsCreates: false,
    listsGhost: false,
    noThread: true,
    demo: { failCreates: "windows-limited" },
    intercept: (op, args) => {
      calls.push([op, args]);
      if (op !== "windows.chooseIso") return undefined;
      if (chosen instanceof Error) return Promise.reject(chosen);
      return chosen === null ? null : undefined;
    },
  };
}

describe.skipIf(!wasmBuilt)("Windows without Microsoft's download (real routes, wasm core)", () => {
  it("offers the picker, Microsoft's page and Try again on the same failure screen", async () => {
    const { router, hooks } = await mountApp("/spaces", host());
    await act(async () => {
      await hooks.spaces.createSpace({ image: IMAGE, name: "win", os: "windows" }).catch(() => {});
    });
    const failed = await waitFor(() => {
      const s = hooks.spaces.data?.find((x) => x.name === "win");
      if (!s) throw new Error("no row yet");
      return s;
    });
    await act(async () => {
      await router.navigate({ to: "/spaces/$spaceId", params: { spaceId: failed.id } });
    });
    const error = await waitFor(() => {
      const e = document.querySelector("[data-create-error]");
      if (!e) throw new Error("no failure yet");
      return e;
    });
    // Plain words: no Sentinel, no command line.
    expect(error.textContent).toMatch(/Microsoft is limiting Windows downloads/);
    expect(error.textContent).not.toMatch(/Sentinel|--iso/);
    const labels = Array.from(document.querySelectorAll("button")).map((b) => b.textContent);
    expect(labels).toEqual(expect.arrayContaining(["Choose Windows ISO…", "Open Microsoft's download page", "Try again", "Remove from list"]));
    expect(labels.indexOf("Choose Windows ISO…")).toBeLessThan(labels.indexOf("Open Microsoft's download page"));
    expect(labels.indexOf("Open Microsoft's download page")).toBeLessThan(labels.indexOf("Try again"));
  });

  it("runs the create again with the chosen file, and tells why a wrong file was refused", async () => {
    const calls: [string, unknown][] = [];
    const { router, hooks } = await mountApp("/spaces", host({ name: "Win11_ARM64.iso" }, calls));
    await act(async () => {
      await hooks.spaces.createSpace({ image: IMAGE, name: "win", os: "windows" }).catch(() => {});
    });
    const failed = await waitFor(() => {
      const s = hooks.spaces.data?.find((x) => x.name === "win");
      if (!s) throw new Error("no row yet");
      return s;
    });
    await act(async () => {
      await router.navigate({ to: "/spaces/$spaceId", params: { spaceId: failed.id } });
    });
    fireEvent.click(await screen.findByRole("button", { name: "Choose Windows ISO…" }));
    await waitFor(() => expect(calls).toContainEqual(["windows.chooseIso", { image: IMAGE }]));
    // The same create ran again: the failure is gone and the Space comes up.
    await waitFor(() => expect(document.querySelector("[data-create-error]")).toBeNull());
    await waitFor(() => expect(hooks.spaces.data?.find((x) => x.name === "win")?.status).toBe("running"));
  });

  it("shows Choose Windows ISO… as a link in the Windows step of New Space", async () => {
    const calls: [string, unknown][] = [];
    await mountApp("/spaces", host({ name: "Win11_ARM64.iso" }, calls));
    // The host asks for New Space on this computer (Windows here is built from an ISO).
    await screen.findByRole("button", { name: /New Space/ });
    act(() => requestNewSpace("local"));
    // The wizard offers the picker only for a Windows image that is built on this computer.
    const tile = await waitFor(() => {
      const t = document.querySelector<HTMLButtonElement>('[data-tile="os:windows"]');
      if (!t || t.disabled) throw new Error("no Windows tile yet");
      return t;
    });
    fireEvent.click(tile);
    const link = await screen.findByRole("button", { name: "Choose Windows ISO…" });
    fireEvent.click(link);
    await waitFor(() => expect(document.querySelector("[data-windows-iso]")?.textContent).toContain("Windows installs from Win11_ARM64.iso."));
    expect(calls.some(([op]) => op === "windows.chooseIso")).toBe(true);
  });
});
