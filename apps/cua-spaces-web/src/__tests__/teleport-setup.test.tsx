// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The teleport setup sheet: what a browser teleport needs, in plain words,
// before anything is read. Keyvault, Full Disk Access, Automation, profile.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RouterProvider, createMemoryHistory, createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import type { KvReadiness, TeleportSetup, TeleportStore } from "@/bridge";
import { demoReadiness } from "@/bridge/adapters/demo/teleport";
import { TeleportSetupSheet } from "@/components/teleport/setup-sheet";

afterEach(cleanup);

function store() {
  return {
    close: vi.fn(),
    openPane: vi.fn(async () => {}),
    useTabsOnly: vi.fn(),
    chooseProfile: vi.fn(async () => {}),
  } as unknown as TeleportStore & Record<string, ReturnType<typeof vi.fn>>;
}

function sheet(readiness: KvReadiness, t: TeleportStore, extra: Partial<TeleportSetup> = {}) {
  const setup: TeleportSetup = {
    open: true,
    checking: false,
    readiness,
    profile: readiness.profile ?? null,
    error: null,
    blocked: !readiness.keyvault_initialized || readiness.files.state === "denied" || readiness.profile_empty,
    ...extra,
  };
  const root = createRootRoute({ component: () => <TeleportSetupSheet setup={setup} teleport={t} /> });
  const keyvault = createRoute({ getParentRoute: () => root, path: "/keyvault", component: () => null });
  const router = createRouter({ routeTree: root.addChildren([keyvault]), history: createMemoryHistory({ initialEntries: ["/"] }) });
  return render(<RouterProvider router={router} />);
}

const row = (key: string) => document.querySelector<HTMLElement>(`[data-setup-row="${key}"]`);
const states = () => [...document.querySelectorAll<HTMLElement>("[data-setup-state]")].map((e) => e.dataset.setupState);

describe("Teleport setup sheet", () => {
  it("says in plain words that Full Disk Access is needed, and offers open tabs only", async () => {
    const t = store();
    sheet(demoReadiness(null, "blocked", 1), t);
    await screen.findByText(/cannot be teleported yet/);
    expect(document.querySelector("[data-teleport-setup]")?.getAttribute("data-teleport-setup")).toBe("blocked");
    expect(states()).toEqual(["Ready", "Needed", "Asked when needed"]);
    const say = document.querySelector("[data-setup-say='files']")!.textContent!;
    expect(say).toContain("Full Disk Access");
    expect(say).toContain("restart the Cua daemon");
    expect(say).not.toContain("os error");
    fireEvent.click(screen.getByRole("button", { name: "Open Settings" }));
    expect(t.openPane).toHaveBeenCalledWith(expect.stringContaining("Privacy_AllFiles"));
    fireEvent.click(screen.getByRole("button", { name: "Send open tabs only" }));
    expect(t.useTabsOnly).toHaveBeenCalled();
  });

  it("lists the profiles with name and account, the last used marked, and chooses one", async () => {
    const t = store();
    sheet(demoReadiness(null, "profiles", 2), t);
    await screen.findByText(/Choose the Google Chrome profile/);
    const profiles = [...document.querySelectorAll<HTMLElement>("[data-setup-profile]")];
    expect(profiles.map((p) => p.dataset.setupProfile)).toEqual(["Default", "Profile 1", "Profile 2"]);
    expect(profiles[1]!.textContent).toContain("Work");
    expect(profiles[1]!.textContent).toContain("ada@design-studio.example");
    expect(profiles[1]!.textContent).toContain("Used last");
    expect(profiles[0]!.textContent).not.toContain("Used last");
    // Nothing blocks, so no Open Settings and no tabs-only.
    expect(screen.queryByRole("button", { name: "Open Settings" })).toBeNull();
    expect(row("tabs-only")).toBeNull();
    fireEvent.click(profiles[2]!.querySelector("[role='radio']")!);
    expect(t.chooseProfile).toHaveBeenCalledWith("Profile 2");
  });

  it("asks for the Keyvault first when it does not exist, and leaves for its page", async () => {
    const t = store();
    sheet({ ...demoReadiness(null, "ready", 2), keyvault_initialized: false }, t);
    await screen.findByText(/Create it first/);
    expect(states()[0]).toBe("Not set up");
    fireEvent.click(screen.getByRole("link", { name: "Set up" }));
    expect(t.close).toHaveBeenCalled();
  });

  it("says a profile with no sign-in data names the profiles that exist", async () => {
    const t = store();
    const r = demoReadiness("Profile 2", "profiles", 2);
    sheet({ ...r, profile_empty: true, files: { state: "granted", say: 'The browser profile "Profile 2" has no sign-in data. Profiles found: Default, Profile 1.' } }, t);
    await screen.findByText(/has no sign-in data/);
    expect(document.querySelector("[data-teleport-setup]")?.getAttribute("data-teleport-setup")).toBe("blocked");
  });

  it("uses words, never a dot between them, and never a device", async () => {
    sheet(demoReadiness(null, "blocked", 1), store());
    await screen.findByText(/cannot be teleported yet/);
    const text = document.body.textContent ?? "";
    expect(text).not.toContain("·");
    expect(text.toLowerCase()).not.toContain("device");
  });
});
