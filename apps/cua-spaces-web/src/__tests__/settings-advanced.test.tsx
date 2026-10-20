// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Remote hours and self-hosting in Settings, on the real routes and the wasm
// core: the Free plan card says what remote hours are; your own relay (or a
// direct connection) is only under Advanced, collapsed, and opens host
// setup's form for it. Setup elsewhere offers Cua's relay alone.

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

describe.skipIf(!wasmBuilt)("remote hours and self-hosting in Settings (real routes, wasm core)", () => {
  it("says what remote hours are where they first show", async () => {
    const { router } = await mountApp("/settings", host);
    await router.load();
    const card = await waitFor(() => {
      const el = document.querySelector("[data-plan-card]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(card.textContent).toContain(
      "Remote hours: time spent using your computers from somewhere else (your laptop, phone or an agent on another computer). Using a computer from itself is free.",
    );
    expect(document.querySelector("[data-relay-usage]")?.textContent).toContain("Remote use");
    expect(document.body.textContent).not.toMatch(/relay hours/i);
  });

  it("keeps your own relay under Advanced, collapsed, for self-hosting teams", async () => {
    const { router } = await mountApp("/settings", host);
    await router.load();
    const section = await waitFor(() => {
      const el = document.querySelector("[data-settings-advanced]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(section.getAttribute("data-settings-advanced")).toBe("closed");
    expect(document.body.textContent).not.toContain("Your own relay");
    fireEvent.click(section.querySelector("button")!);
    const row = await waitFor(() => {
      const el = section.querySelector("[data-self-hosting]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(row.textContent).toContain("For self-hosting teams");
    fireEvent.click(row.querySelector("button")!);
    const form = await waitFor(() => {
      const el = section.querySelector("[data-host-form]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(form.getAttribute("data-host-advanced")).toBe("open");
    expect(form.querySelector("[data-host-lede]")?.textContent).toMatch(/^For self-hosting teams/);
    expect(form.querySelector('[data-host-field="relay"]')).toBeTruthy();
    expect(form.querySelector('[data-host-field="direct"]')).toBeTruthy();
  });
});
