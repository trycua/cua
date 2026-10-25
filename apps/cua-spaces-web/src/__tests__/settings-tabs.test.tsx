// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings' six tabs on the real routes and the wasm core: Account, General,
// Connected apps, Trusted devices, Advanced, About. The old addresses
// (Agents, Experiments) still land on the right tab, and what moved is where
// the audit put it.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { useUiStore } from "@/stores/ui";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(() => {
  cleanup();
  localStorage.clear();
});
stubBrowserApis();

const host: TestHost = { failsCreates: false, listsGhost: false };
const tab = (id: string) => document.querySelector<HTMLElement>(`[data-settings-tab="${id}"]`);
const labels = () => [...document.querySelectorAll<HTMLElement>('nav[aria-label="Settings"] a')].map((a) => a.textContent);

describe.skipIf(!wasmBuilt)("Settings tabs (real routes, wasm core)", () => {
  it("has Account, General, Connected apps, Trusted devices, Advanced and About, in that order", async () => {
    await mountApp("/settings", host);
    await waitFor(() => expect(tab("account")).not.toBeNull());
    expect(labels()).toEqual(["Account", "General", "Connected apps", "Trusted devices", "Advanced", "About"]);
  });

  it.each([
    ["/settings/agents", "/settings/connected-apps", "connected-apps"],
    ["/settings/experiments", "/settings/advanced", "advanced"],
  ])("sends the old %s to %s", async (from, to, id) => {
    const { router } = await mountApp(from, host);
    await waitFor(() => expect(router.state.location.pathname).toBe(to));
    await waitFor(() => expect(tab(id)).not.toBeNull());
  });

  it("keeps General to the everyday settings: no Keyboard shortcuts, Shared links, Storage or Runtimes", async () => {
    await mountApp("/settings/general", host);
    await waitFor(() => expect(tab("general")).not.toBeNull());
    await waitFor(() => expect(document.querySelector('[data-setting-row="telemetry"]')).not.toBeNull());
    const text = tab("general")!.textContent!;
    expect(text).toContain("Start Cua Spaces when you sign in");
    expect(text).toContain("Appearance");
    expect(text).not.toMatch(/Keyboard shortcuts|Shared links|Storage|Runtimes|Keyvault|AI agents|Welcome/);
  });

  it("puts Connected apps where Agents was, with the account keys and Terminal", async () => {
    await mountApp("/settings/connected-apps", host);
    await waitFor(() => expect(document.querySelector("[data-agent-keys]")).not.toBeNull(), { timeout: 8000 });
    const text = tab("connected-apps")!.textContent!;
    expect(text).toContain("Account keys");
    expect(text).toContain("Add a key for another AI company");
    expect(text).not.toMatch(/provider key/i);
  });

  it("moves Early releases here from About, and drops the Sharing and Your cloud switches", async () => {
    await mountApp("/settings/advanced", host);
    await waitFor(() => expect(tab("advanced")).not.toBeNull());
    await waitFor(() => expect(tab("advanced")!.textContent).toContain("Get early releases (Beta)"));
    const text = tab("advanced")!.textContent!;
    expect(text).toContain("Cua Volume");
    expect(text).not.toMatch(/Your cloud|Sharing|Experiments/);
    expect(document.querySelector('[data-setting-row="experiment:sharing"]')).toBeNull();
    expect(document.querySelector('[data-setting-row="experiment:your_cloud"]')).toBeNull();
  });

  it("puts Storage on the Volume page, not in Settings", async () => {
    await mountApp("/settings/advanced", host);
    await waitFor(() => expect(tab("advanced")).not.toBeNull());
    expect(document.querySelector('[data-settings-section="storage"]')).toBeNull();
    cleanup();
    await mountApp("/volume", host);
    await waitFor(() => expect(document.querySelector('[data-settings-section="storage"]')).not.toBeNull(), { timeout: 8000 });
  });

  it("About has the welcome and the update switch, and no Beta picker", async () => {
    await mountApp("/settings/about", host);
    await waitFor(() => expect(document.querySelector("[data-about-welcome]")).not.toBeNull());
    expect(document.body.textContent).toContain("Show the welcome again");
    expect(document.body.textContent).not.toContain("Update to:");
  });
});

describe.skipIf(!wasmBuilt)("Cmd+K finds Settings tabs", () => {
  it("opens Connected apps from the palette", async () => {
    const { router } = await mountApp("/spaces", host);
    await waitFor(() => expect(document.querySelector("nav")).not.toBeNull());
    act(() => useUiStore.getState().setPaletteOpen(true));
    const input = await waitFor(() => {
      const el = document.querySelector<HTMLInputElement>("[data-command-palette] input");
      expect(el).not.toBeNull();
      return el!;
    });
    fireEvent.change(input, { target: { value: "connected apps" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/connected-apps"));
  });
});
