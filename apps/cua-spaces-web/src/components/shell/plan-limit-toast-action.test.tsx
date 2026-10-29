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

/** A core that answers `relayUsage.planLimit`, `upgrade.link` and
 * `telemetry.planLimit` the way the real one does. */
const core: CoreClient = {
  ...noCore,
  status: "ready",
  call: () => undefined as never,
  tryCall: <T,>(method: string, args?: Record<string, unknown>) => {
    if (method === "relayUsage.planLimit")
      return (String(args?.error).includes("live sessions on Pro")
        ? { text: SENTENCE, upgrade: "Upgrade", upgradeUrl: "https://app.cua.ai/buy/teams?from=plan_limit", dismiss: "Dismiss", dismissKey: "refused:Sessions", level: "limit", kind: "sessions", plan: "pro" }
        : null) as T;
    if (method === "upgrade.link") return String(args?.url).replace("from=plan_limit", `from=${String(args?.from)}`) as T;
    if (method === "telemetry.planLimit")
      return [{ type: args?.action === "shown" ? "plan-limit-shown" : "upgrade-link-opened", surface: args?.surface, kind: args?.kind, plan: args?.plan }] as T;
    return null as T;
  },
};

const tracked: unknown[] = [];

function mount() {
  const demo = createDemoAdapter({ latencyMs: 0 });
  const opened: string[] = [];
  tracked.length = 0;
  const adapter: DataAdapter = {
    ...demo,
    call: (op, args) => {
      if (op === "session.openExternal") opened.push((args as { url: string }).url);
      if (op === "telemetry.track") tracked.push(...(args as { signals: unknown[] }).signals);
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
    // The link says where it was opened (`?from=`): opening a Space.
    expect(opened).toEqual(["https://app.cua.ai/buy/teams?from=app_connect"]);
  });

  it("records the limit shown and the upgrade opened, and a create's toast says app_create", async () => {
    const opened = mount();
    await act(() => new Promise((r) => setTimeout(r, 10)));
    act(() => toastError("Couldn't create the Space", "create")(new Error(SENTENCE)));
    fireEvent.click(document.querySelector("[data-toast-action]") as HTMLElement);
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(opened).toEqual(["https://app.cua.ai/buy/teams?from=app_create"]);
    const usage = tracked.filter((s) => ["plan-limit-shown", "upgrade-link-opened"].includes((s as { type: string }).type));
    expect(usage).toEqual([
      { type: "plan-limit-shown", surface: "app_create", kind: "sessions", plan: "pro" },
      { type: "upgrade-link-opened", surface: "app_create", kind: "sessions", plan: "pro" },
    ]);
  });

  it("any other failure has no Upgrade", async () => {
    mount();
    await act(() => new Promise((r) => setTimeout(r, 10)));
    act(() => toastError("Couldn't open the Space")(new Error("timed out")));
    expect(document.querySelector("[data-toast-action]")).toBeNull();
  });
});
