// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The one-time linking notice (CUA-1881): shown once when the host says it is
// due, closed only by its two buttons, and "Turn off usage data" turns the
// switch off through the Settings path before it acknowledges.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { createDemoAdapter } from "@/bridge/adapters/demo";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { unavailableCore } from "@/bridge/core";
import { BridgeProvider, TURN_OFF_FAILED, type DataAdapter, type TelemetryLinkNotice } from "@/bridge";
import { UsageLinkNotice } from "@/components/usage-link-notice";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const NOTICE: TelemetryLinkNotice = {
  due: true,
  title: "We've updated how usage data works",
  body: "Cua collects usage data, like which features you use and when something goes wrong. Until now, it wasn't connected to your account.",
  okLabel: "Got it",
  turnOffLabel: "Turn off usage data",
  detailsLabel: "What is collected",
  docsUrl: "https://cua.ai/docs/cua-sdk/concepts/telemetry",
};

/** The demo host with a linking notice: what it was asked, in order. */
function hostWith(notice: TelemetryLinkNotice | null, options: { failsTurnOff?: boolean } = {}) {
  const demo = createDemoAdapter({ latencyMs: 0 });
  const calls: Array<[string, unknown]> = [];
  const adapter: DataAdapter = {
    mode: demo.mode,
    subscribe: demo.subscribe,
    call: (async (op: string, args: never) => {
      calls.push([op, args]);
      if (op === "telemetry.linkNotice") return notice;
      if (op === "telemetry.acknowledgeLinkNotice") return null;
      if (op === "settings.set" && options.failsTurnOff) throw new Error("config is read-only");
      return demo.call(op as never, args);
    }) as DataAdapter["call"],
    dispose: demo.dispose,
  };
  return { adapter, calls, demo };
}

function mount(adapter: DataAdapter) {
  return render(
    <BridgeProvider adapter={adapter} core={unavailableCore("test: no core")}>
      <UsageLinkNotice />
    </BridgeProvider>,
  );
}

const ops = (calls: Array<[string, unknown]>) => calls.map(([op]) => op);

describe("the one-time usage-data linking notice", () => {
  it("shows the host's words with a link to what is collected when it is due", async () => {
    const { adapter } = hostWith(NOTICE);
    mount(adapter);
    expect(await screen.findByText(NOTICE.title)).toBeTruthy();
    expect(screen.getByText(NOTICE.body)).toBeTruthy();
    expect(screen.getByRole("button", { name: NOTICE.okLabel })).toBeTruthy();
    expect(screen.getByRole("button", { name: NOTICE.turnOffLabel })).toBeTruthy();
    expect(screen.getByRole("button", { name: NOTICE.detailsLabel })).toBeTruthy();
  });

  it("is not shown when it is not due, when the host has none, or when it cannot say", async () => {
    for (const notice of [{ ...NOTICE, due: false }, null]) {
      const { adapter, calls } = hostWith(notice);
      const view = mount(adapter);
      await waitFor(() => expect(ops(calls)).toContain("telemetry.linkNotice"));
      expect(screen.queryByText(NOTICE.title)).toBeNull();
      view.unmount();
    }
    const demo = createDemoAdapter({ latencyMs: 0 });
    const failing = { ...demo, call: (async (op: string, args: never) => {
      if (op === "telemetry.linkNotice") throw new Error("host is busy");
      return demo.call(op as never, args);
    }) as DataAdapter["call"], subscribe: demo.subscribe } as DataAdapter;
    mount(failing);
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.queryByText(NOTICE.title)).toBeNull();
  });

  it("the demo host has no notice", async () => {
    const demo = createDemoAdapter({ latencyMs: 0 });
    expect(await demo.call("telemetry.linkNotice", {})).toBeNull();
    expect(await demo.call("telemetry.acknowledgeLinkNotice", {})).toBeNull();
    demo.dispose?.();
  });

  it("Got it acknowledges and closes it, leaving usage data on", async () => {
    const { adapter, calls, demo } = hostWith(NOTICE);
    mount(adapter);
    fireEvent.click(await screen.findByRole("button", { name: NOTICE.okLabel }));
    await waitFor(() => expect(screen.queryByText(NOTICE.title)).toBeNull());
    expect(ops(calls)).toContain("telemetry.acknowledgeLinkNotice");
    expect(calls.some(([op]) => op === "settings.set")).toBe(false);
    expect((await demo.call("settings.get", {})).values.telemetry).toBe(true);
  });

  it("Turn off usage data turns it off through settings, then acknowledges and closes", async () => {
    const { adapter, calls, demo } = hostWith(NOTICE);
    mount(adapter);
    fireEvent.click(await screen.findByRole("button", { name: NOTICE.turnOffLabel }));
    await waitFor(() => expect(screen.queryByText(NOTICE.title)).toBeNull());
    const set = calls.findIndex(([op]) => op === "settings.set");
    const ack = calls.findIndex(([op]) => op === "telemetry.acknowledgeLinkNotice");
    expect(calls[set]).toEqual(["settings.set", { key: "telemetry", value: false }]);
    expect(ack).toBeGreaterThan(set);
    expect((await demo.call("settings.get", {})).values.telemetry).toBe(false);
  });

  it("stays up, without acknowledging, when usage data could not be turned off", async () => {
    const { adapter, calls } = hostWith(NOTICE, { failsTurnOff: true });
    mount(adapter);
    fireEvent.click(await screen.findByRole("button", { name: NOTICE.turnOffLabel }));
    expect((await screen.findByRole("alert")).textContent).toBe(TURN_OFF_FAILED);
    expect(screen.getByText(NOTICE.title)).toBeTruthy();
    expect(ops(calls)).not.toContain("telemetry.acknowledgeLinkNotice");
  });

  it("Escape does not close it: only the two buttons are a choice", async () => {
    const { adapter, calls } = hostWith(NOTICE);
    mount(adapter);
    await screen.findByText(NOTICE.title);
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    await new Promise((r) => setTimeout(r, 30));
    expect(screen.getByText(NOTICE.title)).toBeTruthy();
    expect(ops(calls)).not.toContain("telemetry.acknowledgeLinkNotice");
  });
});

describe.skipIf(!wasmBuilt)("the notice in the app shell (real routes, wasm core)", () => {
  const due = (asked: string[]) => ({
    failsCreates: false,
    listsGhost: false,
    intercept: (op: string) => {
      asked.push(op);
      if (op === "telemetry.linkNotice") return NOTICE;
      if (op === "telemetry.acknowledgeLinkNotice") return null;
      return undefined;
    },
  });

  it("shows over the app when the host says it is due, and Got it closes it", async () => {
    const asked: string[] = [];
    const { router } = await mountApp("/", due(asked));
    await router.load();
    fireEvent.click(await screen.findByRole("button", { name: NOTICE.okLabel }));
    await waitFor(() => expect(screen.queryByText(NOTICE.title)).toBeNull());
    expect(asked).toContain("telemetry.acknowledgeLinkNotice");
  });

  it("does not interrupt the first run", async () => {
    const asked: string[] = [];
    const { router } = await mountApp("/onboarding", due(asked));
    await router.load();
    await new Promise((r) => setTimeout(r, 100));
    expect(screen.queryByText(NOTICE.title)).toBeNull();
    expect(asked).not.toContain("telemetry.linkNotice");
  });
});
