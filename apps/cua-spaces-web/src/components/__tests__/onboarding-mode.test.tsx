// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { BridgeProvider, useOnboarding, useSession, useThisMachine, type OnboardingHook } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { readSavedOnboarding, resetSavedOnboardingCache } from "@/bridge/onboarding";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { ModeCard } from "@/components/onboarding-mode";

beforeEach(() => {
  localStorage.clear();
  resetSavedOnboardingCache();
});
afterEach(cleanup);

const probe = {} as { flow: OnboardingHook };

function Page() {
  const flow = useOnboarding();
  const { data: session } = useSession();
  const host = useThisMachine(session?.identity ?? null);
  probe.flow = flow;
  if (flow.view?.step !== "mode") return null;
  return <ModeCard flow={flow} view={flow.view} host={host} />;
}

describe.skipIf(!wasmBuilt)("This machine in the first run", () => {
  it("opens host setup from the host choice instead of a dead end, and moves on once it worked", async () => {
    const adapter = createDemoAdapter({ latencyMs: 1, stepMs: 1, signedIn: true, onboarded: false });
    render(
      <BridgeProvider adapter={adapter} core={await testCore()} storeOptions={{ tickMs: 5 }}>
        <Page />
      </BridgeProvider>,
    );
    await waitFor(() => expect(probe.flow.state).toBeTruthy());
    act(() => probe.flow.send({ type: "start" }));
    act(() => probe.flow.send({ type: "signin-done" }));
    act(() => probe.flow.send({ type: "agents-done", configured: [] }));
    const hostChoice = await screen.findByTestId("onboarding-host");
    expect(hostChoice).not.toHaveProperty("disabled", true);
    expect(screen.queryByText(/Available once this machine is set up/)).toBeNull();
    expect(hostChoice.textContent).toMatch(/Next: name this machine/);

    fireEvent.click(hostChoice);
    const form = await waitFor(() => {
      const f = document.querySelector("[data-host-form]");
      if (!f) throw new Error("no host form");
      return f as HTMLFormElement;
    });
    await waitFor(() => expect(form.querySelector<HTMLButtonElement>("[data-host-submit]")?.disabled).toBe(false));
    fireEvent.submit(form);
    await waitFor(() => expect(readSavedOnboarding().state?.step).toBe("done"), { timeout: 5000 });
    expect(readSavedOnboarding().state?.mode).toBe("host");
  });

  it("finishes the page at once for Access other machines", async () => {
    const adapter = createDemoAdapter({ latencyMs: 1, stepMs: 1, signedIn: true, onboarded: false });
    render(
      <BridgeProvider adapter={adapter} core={await testCore()} storeOptions={{ tickMs: 5 }}>
        <Page />
      </BridgeProvider>,
    );
    await waitFor(() => expect(probe.flow.state).toBeTruthy());
    act(() => probe.flow.send({ type: "start" }));
    act(() => probe.flow.send({ type: "signin-done" }));
    act(() => probe.flow.send({ type: "agents-done", configured: [] }));
    fireEvent.click(await screen.findByTestId("onboarding-client"));
    expect(readSavedOnboarding().state).toMatchObject({ step: "done", mode: "client" });
  });
});
