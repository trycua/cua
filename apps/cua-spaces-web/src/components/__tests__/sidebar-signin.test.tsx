// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { BridgeProvider } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { unavailableCore } from "@/bridge/core";
import { AccountFooter } from "@/components/shell/sidebar";

afterEach(cleanup);

/** A signed-out demo host whose browser sign-in waits (as a real one does). */
function host() {
  const base = createDemoAdapter({ latencyMs: 0, signedIn: false, onboarded: true });
  const opened: string[] = [];
  const adapter = Object.assign(Object.create(base) as DataAdapter, {
    call: ((op: string, args: Record<string, unknown>) => {
      if (op === "session.signIn") {
        return Promise.resolve({ method: "device", verificationUri: "https://cua.ai/device", userCode: "WXYZ-1234" });
      }
      if (op === "session.openExternal") {
        opened.push(String(args.url));
        return Promise.resolve(null);
      }
      return (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args);
    }) as DataAdapter["call"],
  });
  return { adapter, opened };
}

describe("the sidebar's sign-in", () => {
  it("shows the code, the page to open again and Cancel while it waits, as the first run does", async () => {
    const { adapter, opened } = host();
    render(
      <BridgeProvider adapter={adapter} core={unavailableCore("test: no core")}>
        <AccountFooter />
      </BridgeProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Sign in" }));
    const status = await screen.findByRole("status");
    await waitFor(() => expect(status.textContent).toContain("WXYZ-1234"));
    fireEvent.click(screen.getByText("Open the browser again"));
    await waitFor(() => expect(opened).toEqual(["https://cua.ai/device"]));
    fireEvent.click(screen.getByText("Cancel"));
    expect(await screen.findByRole("button", { name: "Sign in" })).toBeTruthy();
    expect(screen.queryByText("Open the browser again")).toBeNull();
  });
});
