// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The sidebar's account area (CUA-2204) on the real routes, the wasm core
// and the demo host: one line (the initial and the email) that opens
// Settings, Plan (with its word) and Sign out; no "Signed in as", no plan
// line, no sign-out icon. On Free only, "Run more Spaces at once with Pro"
// and Upgrade above it, with no way to close it; a paid plan never sees it.

import { cleanup, fireEvent, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const sidebar = () => document.querySelector<HTMLElement>("aside[aria-label=Sidebar]")!;
const line = () => sidebar().querySelector<HTMLElement>("[data-account-line]");
const banner = () => sidebar().querySelector<HTMLElement>("[data-pro-banner]");

function host(usage: NonNullable<TestHost["demo"]>["usage"], extra: Partial<TestHost> = {}) {
  const opened: string[] = [];
  const tracked: unknown[] = [];
  const h: TestHost = {
    failsCreates: false,
    listsGhost: false,
    ...extra,
    demo: { usage, ...extra.demo },
    intercept: (op, args) => {
      if (op === "session.openExternal") return (opened.push((args as { url: string }).url), null);
      if (op === "telemetry.track") tracked.push(...((args as { signals: unknown[] }).signals ?? []));
      return extra.intercept?.(op, args);
    },
  };
  return { h, opened, tracked };
}

const openMenu = async () => {
  const trigger = await waitFor(() => {
    const el = line();
    if (!el) throw new Error("no account line");
    return el;
  });
  fireEvent.click(trigger);
  return waitFor(() => {
    const menu = document.querySelector<HTMLElement>("[data-account-menu]");
    if (!menu) throw new Error("no account menu");
    return menu;
  });
};

const items = (menu: HTMLElement) => Array.from(menu.querySelectorAll<HTMLElement>("[data-account-item]")).map((i) => i.textContent);

describe.skipIf(!wasmBuilt)("the account area", () => {
  it("is one line: the initial and the email, nothing else", async () => {
    const { h } = host("pro");
    await mountApp("/spaces", h);
    const l = await waitFor(() => line() ?? Promise.reject(new Error("no line")));
    expect(l.textContent).toContain("ada@example.com");
    const footer = sidebar().querySelector<HTMLElement>("[data-signin=signedIn]")!;
    expect(footer.textContent).not.toContain("Signed in as");
    expect(footer.querySelector("[data-account-plan]")).toBeNull();
    expect(within(footer).queryByRole("button", { name: "Sign out" })).toBeNull();
  });

  it("opens Settings, Plan with its word, and Sign out", async () => {
    const { h, opened, tracked } = host("idle");
    const app = await mountApp("/spaces", h);
    const menu = await openMenu();
    expect(items(menu)).toEqual(["Settings", "PlanFree", "Sign out"]);
    expect(menu.querySelectorAll("[role=separator]").length).toBe(1);
    fireEvent.click(within(menu).getByText("Plan"));
    await waitFor(() => expect(opened[0]).toMatch(/from=app_account_menu/));
    // `upsell_account_menu_click`, as before.
    await waitFor(() => expect(JSON.stringify(tracked)).toContain("account_menu"));
    fireEvent.click(within(await openMenu()).getByText("Settings"));
    await waitFor(() => expect(app.router.state.location.pathname).toBe("/settings"));
    fireEvent.click(within(await openMenu()).getByText("Sign out"));
    await waitFor(() => expect(sidebar().querySelector("[data-signin=signedOut], [data-signin=idle]")).not.toBeNull());
  });

  it("names the paid plan and shows no banner", async () => {
    const { h } = host("teams");
    await mountApp("/spaces", h);
    const menu = await openMenu();
    expect(items(menu)).toContain("PlanTeams");
    expect(banner()).toBeNull();
  });

  it("shows the Pro line on Free; Upgrade opens the plan and nothing closes it", async () => {
    const { h, opened } = host("idle");
    await mountApp("/spaces", h);
    const b = await waitFor(() => banner() ?? Promise.reject(new Error("no banner")));
    expect(b.textContent).toContain("Run more Spaces at once with Pro");
    const copy = b.querySelector<HTMLElement>("[data-pro-copy]")!;
    expect(copy.className).toContain("whitespace-normal");
    expect(copy.className).not.toContain("truncate");
    expect(within(b).queryByRole("button", { name: "Dismiss" })).toBeNull();
    expect(b.querySelector("[data-pro-dismiss]")).toBeNull();
    fireEvent.click(within(b).getByRole("button", { name: "Upgrade" }));
    await waitFor(() => expect(opened[0]).toMatch(/from=app_account_menu/));
    expect(banner()).not.toBeNull();
  });

  it("shows it even near a Free limit", async () => {
    const { h } = host("warn");
    await mountApp("/spaces", h);
    await waitFor(() => expect(banner()).not.toBeNull());
  });
});
