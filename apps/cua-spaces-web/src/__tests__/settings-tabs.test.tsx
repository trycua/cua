// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings' five tabs on the real routes and the wasm core: Account, General,
// Connections, Advanced, About. The old addresses (Agents, Experiments) still
// land on the right tab, the old Trusted machines tab lands on Machines, and
// what moved is where the audit put it.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useUiStore } from "@/stores/ui";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(() => {
  vi.unstubAllEnvs();
  cleanup();
  localStorage.clear();
});
stubBrowserApis();

const host: TestHost = { failsCreates: false, listsGhost: false };
const tab = (id: string) => document.querySelector<HTMLElement>(`[data-settings-tab="${id}"]`);
const labels = () => [...document.querySelectorAll<HTMLElement>('nav[aria-label="Settings"] a')].map((a) => a.textContent);

describe.skipIf(!wasmBuilt)("Settings tabs (real routes, wasm core)", () => {
  it("has Account, General, Connections, Advanced and About, in that order", async () => {
    await mountApp("/settings", host);
    await waitFor(() => expect(tab("account")).not.toBeNull());
    expect(labels()).toEqual(["Account", "General", "Connections", "Advanced", "About"]);
  });

  it.each([
    ["/settings", "Account"],
    ["/settings/general", "General"],
    ["/settings/connections", "Connections"],
    ["/settings/advanced", "Advanced"],
    ["/settings/about", "About"],
  ])("marks only the tab for %s as selected (%s)", async (path, label) => {
    await mountApp(path, host);
    const tabs = () => [...document.querySelectorAll<HTMLElement>('[role="tablist"][aria-label="Settings"] [role="tab"]')];
    await waitFor(() => expect(tabs().filter((t) => t.getAttribute("aria-selected") === "true").map((t) => t.textContent)).toEqual([label]));
    expect(tabs().filter((t) => t.getAttribute("aria-selected") === "false")).toHaveLength(4);
  });

  it.each([
    ["/settings/agents", "/settings/connections", "connections"],
    ["/settings/connected-apps", "/settings/connections", "connections"],
    ["/settings/connectors", "/settings/connections", "connections"],
    ["/settings/experiments", "/settings/advanced", "advanced"],
  ])("sends the old %s to %s", async (from, to, id) => {
    const { router } = await mountApp(from, host);
    await waitFor(() => expect(router.state.location.pathname).toBe(to));
    await waitFor(() => expect(tab(id)).not.toBeNull());
  });

  it("sends an old app address under Connectors to the same app under Connections", async () => {
    const { router } = await mountApp("/settings/connectors/hermes", host);
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/connections/hermes"));
    await waitFor(() => expect(document.querySelector("[data-connector-detail=hermes]")).not.toBeNull());
  });

  it("sends the old Trusted machines address to the Machines tab", async () => {
    const { router } = await mountApp("/settings/devices", host);
    await waitFor(() => expect(router.state.location.pathname).toBe("/machines"));
    expect(document.querySelector('[data-settings-tab]')).toBeNull();
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

  it("puts the Providers below Coding apps and On another machine, and Terminal is not on this tab", async () => {
    await mountApp("/settings/connections", { ...host, demo: { cli: "missing" } });
    await waitFor(() => expect(document.querySelector("[data-connectors-providers]")).not.toBeNull(), { timeout: 8000 });
    const t = tab("connections")!;
    const headings = [...t.querySelectorAll("h2")].map((h) => h.textContent);
    expect(headings).toEqual(["Coding apps", "On another machine", "Providers"]);
    // The providers are the last section on the page.
    const sections = [...t.querySelectorAll("[data-connectors-apps], [data-connectors-other-machine-group], [data-connectors-providers]")].map((e) => e.getAttributeNames().find((n) => n.startsWith("data-connectors-")));
    expect(sections).toEqual(["data-connectors-apps", "data-connectors-other-machine-group", "data-connectors-providers"]);
    expect(t.textContent).not.toContain("Connections");
    expect(t.textContent).not.toMatch(/Use Cua from Terminal/);
    expect(t.querySelector("[data-settings-cli]")).toBeNull();
    expect(document.querySelector("[data-agent-key-add-other]")).not.toBeNull();
    expect(t.textContent).not.toMatch(/provider key/i);
  });

  it("keeps Use Cua from Terminal reachable, in General", async () => {
    await mountApp("/settings/general", { ...host, demo: { cli: "missing" } });
    await waitFor(() => expect(document.querySelector('[data-settings-cli="missing"]')).not.toBeNull(), { timeout: 8000 });
    expect(tab("general")!.textContent).toContain("Use Cua from Terminal");
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
  it("opens Connections from the palette, by its old name too", async () => {
    const { router } = await mountApp("/spaces", host);
    await waitFor(() => expect(document.querySelector("nav")).not.toBeNull());
    act(() => useUiStore.getState().setPaletteOpen(true));
    const input = await waitFor(() => {
      const el = document.querySelector<HTMLInputElement>("[data-command-palette] input");
      expect(el).not.toBeNull();
      return el!;
    });
    fireEvent.change(input, { target: { value: "connectors" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/connections"));
  });
});
