// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "Connected" must mean valid (CUA-2680): opening Settings, Connections or the Agents page asks the
// providers whether the saved sign-ins and keys still work; a refused one reads "Needs reconnect" with
// a Reconnect button, on its provider row and on the apps that read it. Reconnecting clears it.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetConnectState } from "@/hooks/use-connectors";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetConnectedAppsState();
  resetConnectState();
});
afterEach(cleanup);

const q = (s: string) => document.querySelector<HTMLElement>(s);
const found = (s: string) =>
  waitFor(() => {
    const el = q(s);
    expect(el).not.toBeNull();
    return el!;
  });
const click = (el: Element) =>
  act(async () => {
    fireEvent.click(el);
  });

function host(calls: string[], rejected: string[] = []) {
  return { failsCreates: false, listsGhost: false, demo: { rejectedKeys: rejected }, intercept: (op: string) => (calls.push(op), undefined) };
}

describe.skipIf(!wasmBuilt)("a saved sign-in the provider refuses", () => {
  it("is re-checked when Settings, Connections opens, and reads Needs reconnect with Reconnect", async () => {
    const calls: string[] = [];
    await mountApp("/settings/connections", host(calls, ["OPENAI_API_KEY"]));
    await waitFor(() => expect(q("[data-connector-provider=openai]")?.textContent).toContain("Needs reconnect"));
    expect(calls).toContain("agentKeys.check");
    const row = q("[data-connector-provider=openai]")!;
    expect(row.dataset.state).toBe("not-connected");
    expect(row.querySelector("[data-connector-provider-action]")?.textContent).toBe("Reconnect");
    // Not "Connected", and no Disconnect for a credential that does not work.
    expect(row.textContent).not.toContain("Disconnect");
    expect(row.querySelector("[data-connector-status]")?.textContent).not.toContain("Connected");
  });

  it("is re-checked when the Agents page opens", async () => {
    const calls: string[] = [];
    await mountApp("/agents", host(calls));
    await waitFor(() => expect(calls).toContain("agentKeys.check"));
  });

  it("a key the provider accepts stays Connected", async () => {
    const calls: string[] = [];
    await mountApp("/settings/connections", host(calls));
    await waitFor(() => expect(calls).toContain("agentKeys.check"));
    await waitFor(() => expect(q("[data-connector-provider=openai]")?.dataset.state).toBe("connected"));
    expect(q("[data-connector-provider=openai]")!.textContent).not.toContain("Needs reconnect");
  });

  it("Reconnect saves the key again and the row is Connected", async () => {
    const calls: string[] = [];
    await mountApp("/settings/connections", host(calls, ["OPENAI_API_KEY"]));
    await waitFor(() => expect(q("[data-connector-provider=openai]")?.textContent).toContain("Needs reconnect"));
    await click(q("[data-connector-provider=openai] [data-connector-provider-action]")!);
    await found("[data-provider-sheet=openai]");
    await click(q("[data-provider-way=key]")!);
    fireEvent.change(await found("[data-provider-key]"), { target: { value: "sk-openai-new-0000-9z9z" } });
    await click(q("[data-provider-save]")!);
    await waitFor(() => expect(q("[data-connector-provider=openai]")?.dataset.state).toBe("connected"));
    expect(q("[data-connector-provider=openai]")!.textContent).not.toContain("Needs reconnect");
  });
});
