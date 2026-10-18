// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { BridgeProvider, useOnboarding, type OnboardingHook } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { resetSavedOnboardingCache } from "@/bridge/onboarding";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { PresentationCards } from "@/components/onboarding-presentation";

beforeEach(() => {
  localStorage.clear();
  resetSavedOnboardingCache();
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const probe = {} as { flow: OnboardingHook };

function Page() {
  const flow = useOnboarding();
  probe.flow = flow;
  if (flow.view?.step !== "presentation") return null;
  return <PresentationCards flow={flow} view={flow.view} />;
}

async function toMenuBarPage() {
  const adapter = createDemoAdapter({ latencyMs: 1, stepMs: 1, signedIn: false, onboarded: false });
  render(
    <BridgeProvider adapter={adapter} core={await testCore()} storeOptions={{ tickMs: 5 }}>
      <Page />
    </BridgeProvider>,
  );
  await waitFor(() => expect(probe.flow.state).toBeTruthy());
  act(() => probe.flow.send({ type: "start" }));
  act(() => probe.flow.send({ type: "signin-done" }));
  act(() => probe.flow.send({ type: "agents-done", configured: [] }));
  return adapter;
}

describe.skipIf(!wasmBuilt)("Menu bar in the first run", () => {
  it("offers the notch on a Mac and writes the pick to the setting", async () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    const adapter = await toMenuBarPage();
    const notch = await screen.findByTestId("onboarding-notch");
    const menuBar = screen.getByTestId("onboarding-menu-bar");
    expect(notch.textContent).toBe("Notch and menu bar");
    expect(notch.getAttribute("aria-checked")).toBe("true");
    expect(screen.getByRole("img", { name: "The menu bar menu alone" })).toBeTruthy();

    fireEvent.click(menuBar);
    await waitFor(() => expect(adapter.state.settings.menuBar).toBe(true));
    expect(screen.getByTestId("onboarding-menu-bar").getAttribute("aria-checked")).toBe("true");
  });

  it("passes through off a Mac", async () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("Win32");
    await toMenuBarPage();
    expect(probe.flow.view?.step).toBe("mode");
    expect(screen.queryByTestId("onboarding-notch")).toBeNull();
  });
});
