// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { createDemoAdapter } from "@/bridge/adapters/demo";
import { BridgeProvider, type CoreClient, type DataAdapter } from "@/bridge";
import { noCore } from "@/bridge/__tests__/testCore";
import { ToastProvider, toastError, toastManager } from "@/components/ui/toast";

import { PlanLimitToastAction } from "./plan-limit-toast-action";

afterEach(() => {
  act(() => toastManager.close());
  cleanup();
});

const SENTENCE = "You're using 5 of 5 live sessions on Pro. Close one or upgrade to Teams.";

/** A core that answers only `relayUsage.planLimit`, as the real one does. */
const core: CoreClient = {
  ...noCore,
  status: "ready",
  call: () => undefined as never,
  tryCall: <T,>(method: string, args?: Record<string, unknown>) =>
    (method === "relayUsage.planLimit" && String(args?.error).includes("live sessions on Pro")
      ? { text: SENTENCE, upgrade: "Upgrade", upgradeUrl: "https://app.cua.ai/buy/teams?from=plan_limit", dismiss: "Dismiss", dismissKey: "refused:Sessions", level: "limit" }
      : null) as T,
};

function mount() {
  const demo = createDemoAdapter({ latencyMs: 0 });
  const opened: string[] = [];
  const adapter: DataAdapter = {
    ...demo,
    call: (op, args) => {
      if (op === "session.openExternal") opened.push((args as { url: string }).url);
      return demo.call(op, args);
    },
  };
  render(
    <BridgeProvider adapter={adapter} core={core}>
      <ToastProvider>
        <PlanLimitToastAction />
      </ToastProvider>
    </BridgeProvider>,
  );
  return opened;
}

describe("PlanLimitToastAction", () => {
  it("a refused session shows the plain sentence with an Upgrade that opens the plan's page", async () => {
    const opened = mount();
    await act(() => new Promise((r) => setTimeout(r, 10)));
    act(() => toastError("Couldn't open the Space")(new Error(SENTENCE)));
    expect(screen.getAllByText(SENTENCE).length).toBeGreaterThan(0);
    const button = document.querySelector("[data-toast-action]") as HTMLElement;
    expect(button.textContent).toBe("Upgrade");
    fireEvent.click(button);
    await act(() => new Promise((r) => setTimeout(r, 10)));
    expect(opened).toEqual(["https://app.cua.ai/buy/teams?from=plan_limit"]);
  });

  it("any other failure has no Upgrade", async () => {
    mount();
    await act(() => new Promise((r) => setTimeout(r, 10)));
    act(() => toastError("Couldn't open the Space")(new Error("timed out")));
    expect(document.querySelector("[data-toast-action]")).toBeNull();
  });
});
