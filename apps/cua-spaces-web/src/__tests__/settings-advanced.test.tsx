// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Account usage and self-hosting in Settings, on the real routes and the wasm
// core: the usage card says what the time away from this computer is; your own
// connection server (or a direct connection) is only under Advanced, and opens
// host setup's form for it. Setup elsewhere offers Cua's server alone.

import { cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { demoRelayUsage } from "@/bridge/adapters/demo/relay-usage";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(() => {
  cleanup();
  localStorage.clear();
});
stubBrowserApis();

const host: TestHost = {
  failsCreates: false,
  listsGhost: false,
  answer: (op, out) => (op === "relay.usage" ? { ...demoRelayUsage(), liveSessions: 0 } : op === "agents.runs" ? [] : out),
};

describe.skipIf(!wasmBuilt)("account usage and self-hosting in Settings (real routes, wasm core)", () => {
  it("says what the time away from this computer is, in plain words", async () => {
    const { router } = await mountApp("/settings", host);
    await router.load();
    const usage = await waitFor(() => {
      const el = document.querySelector("[data-relay-usage]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(usage.textContent).toContain("Time away from this computer");
    expect(usage.textContent).toContain("away from the computer you're using");
    expect(document.body.textContent).not.toMatch(/relay hours|remote hours/i);
  });

  it("keeps your own connection server under Advanced, for teams that run their own", async () => {
    const { router } = await mountApp("/settings/advanced", host);
    await router.load();
    const row = await waitFor(() => {
      const el = document.querySelector("[data-self-hosting]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(row.textContent).toContain("Use your own connection server");
    expect(row.textContent).toContain("For teams that run their own.");
    expect(document.body.textContent).not.toMatch(/relay/i);
    fireEvent.click(row.querySelector("button")!);
    const form = await waitFor(() => {
      const el = document.querySelector("[data-host-form]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(form.getAttribute("data-host-advanced")).toBe("open");
    expect(form.querySelector("[data-host-lede]")?.textContent).toMatch(/^For self-hosting teams/);
    expect(form.querySelector('[data-host-field="relay"]')).toBeTruthy();
    expect(form.querySelector('[data-host-field="direct"]')).toBeTruthy();
  });
});
