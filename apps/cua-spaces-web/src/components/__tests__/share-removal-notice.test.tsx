// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { BridgeProvider } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import type { CoreClient } from "@/bridge/core";
import { ShareRemovalNotice } from "@/components/shell/share-removal-notice";

afterEach(cleanup);

const NOTE = {
  text: "alex@x.test and sam@x.test can no longer open “Design”, because they're not in your team.",
  action: { label: "Invite to your team", url: "https://app.cua.ai/team" },
  dismiss: "Dismiss",
};

/** The core's `share.removalNote`, as far as these tests need it. */
const core = (): CoreClient => ({
  status: "ready",
  methods: ["share.removalNote"],
  call: () => {
    throw new Error("unused");
  },
  tryCall: <T,>(method: string, args: Record<string, unknown> = {}) =>
    (method === "share.removalNote" && (args.removals as unknown[]).length > 0 ? NOTE : null) as T,
});

function host(removals: { who: string; machineName: string }[], signedIn = true) {
  const base = createDemoAdapter({ latencyMs: 0, stepMs: 0, signedIn, onboarded: true });
  const calls: string[] = [];
  const adapter = Object.assign(Object.create(base) as DataAdapter, {
    call: ((op: string, args: unknown) => {
      if (op === "sharing.removals") {
        calls.push(op);
        return Promise.resolve(removals);
      }
      if (op === "sharing.ackRemovals") {
        calls.push(op);
        return Promise.resolve(null);
      }
      return (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args);
    }) as DataAdapter["call"],
  });
  return { adapter, calls };
}

const mount = (adapter: DataAdapter) =>
  render(
    <BridgeProvider adapter={adapter} core={core()}>
      <ShareRemovalNotice />
    </BridgeProvider>,
  );

describe("The note about people removed from shared computers", () => {
  it("shows one line with an invite button, and acknowledges it once dismissed", async () => {
    const { adapter, calls } = host([
      { who: "alex@x.test", machineName: "Design" },
      { who: "sam@x.test", machineName: "Design" },
    ]);
    mount(adapter);
    await waitFor(() => expect(screen.getByText(NOTE.text)).toBeTruthy());
    expect(screen.getByText("Invite to your team")).toBeTruthy();
    fireEvent.click(screen.getByLabelText("Dismiss"));
    await waitFor(() => expect(calls).toEqual(["sharing.removals", "sharing.ackRemovals"]));
    expect(document.querySelector("[data-share-removal]")).toBeNull();
  });

  it("shows nothing when nobody was removed", async () => {
    const { adapter, calls } = host([]);
    mount(adapter);
    await act(() => new Promise((r) => setTimeout(r, 20)));
    expect(calls).toEqual(["sharing.removals"]);
    expect(document.querySelector("[data-share-removal]")).toBeNull();
  });

  it("does not ask while signed out", async () => {
    const { adapter, calls } = host([{ who: "alex@x.test", machineName: "Design" }], false);
    mount(adapter);
    await act(() => new Promise((r) => setTimeout(r, 20)));
    expect(calls).toEqual([]);
  });
});
