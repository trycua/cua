// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.
// Before the redesign: public's test for this page, run with `spaces_redesign` off
// (the drift guard that keeps the default UI working).

import { vi as __vi } from "vitest";
__vi.stubEnv("VITE_CUA_FLAG_SPACES_REDESIGN", "0");
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { BridgeProvider } from "@/bridge";
import { HostError, type DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { unavailableCore } from "@/bridge/core";
import { JoinWithCodeDialog } from "@/legacy/components/join-with-code-dialog";

afterEach(cleanup);

/** The demo host, recording `host.joinWithCode` and optionally answering it differently. */
function host(override?: (args: Record<string, unknown>) => Promise<unknown>) {
  const base = createDemoAdapter({ latencyMs: 0, stepMs: 0, signedIn: true, onboarded: true });
  const joins: Record<string, unknown>[] = [];
  const adapter = Object.assign(Object.create(base) as DataAdapter, {
    call: ((op: string, args: Record<string, unknown>) => {
      if (op === "host.joinWithCode") {
        joins.push(args);
        if (override) return override(args);
      }
      return (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args);
    }) as DataAdapter["call"],
  });
  return { adapter, joins };
}

async function mount(adapter: DataAdapter, props: { initialCode?: string; initialRelay?: string | null } = {}) {
  render(
    <BridgeProvider adapter={adapter} core={unavailableCore("test: no core")}>
      <JoinWithCodeDialog open onOpenChange={() => {}} {...props} />
    </BridgeProvider>,
  );
  await act(() => new Promise((r) => setTimeout(r, 10)));
}

const state = () => document.querySelector("[data-join-with-code]")?.getAttribute("data-join-with-code");
const field = () => screen.getByLabelText("Pairing code") as HTMLInputElement;
const join = () => screen.getByRole("button", { name: "Join" }) as HTMLButtonElement;

describe("Join with a code", () => {
  it("keeps Join off until the text is a code, then joins with the canonical code", async () => {
    const { adapter, joins } = host();
    await mount(adapter);
    expect(join().disabled).toBe(true);
    fireEvent.change(field(), { target: { value: "k7qm-4xpb" } });
    expect(join().disabled).toBe(true);
    fireEvent.change(field(), { target: { value: "k7qm-4xpb-2rhd" } });
    expect(join().disabled).toBe(false);
    fireEvent.click(join());
    await waitFor(() => expect(state()).toBe("joined"));
    expect(joins).toEqual([{ code: "K7QM4XPB2RHD", relay: null }]);
    expect(document.querySelector("[data-join-joined]")?.textContent).toContain("joined");
  });

  it("fills a code that came from a link and waits for Join", async () => {
    const { adapter, joins } = host();
    await mount(adapter, { initialCode: "K7QM4XPB2RHD", initialRelay: "https://relay.example" });
    expect(field().value).toBe("K7QM-4XPB-2RHD");
    expect(document.querySelector("[data-join-relay]")?.textContent).toContain("https://relay.example");
    expect(joins).toEqual([]);
    fireEvent.click(join());
    await waitFor(() => expect(state()).toBe("joined"));
    expect(joins).toEqual([{ code: "K7QM4XPB2RHD", relay: "https://relay.example" }]);
  });

  it("takes the relay from a pasted link", async () => {
    const { adapter, joins } = host();
    await mount(adapter);
    fireEvent.change(field(), { target: { value: "https://cua.ai/add/K7QM-4XPB-2RHD?relay=https%3A%2F%2Frelay.example" } });
    fireEvent.click(join());
    await waitFor(() => expect(state()).toBe("joined"));
    expect(joins).toEqual([{ code: "K7QM4XPB2RHD", relay: "https://relay.example" }]);
  });

  it("shows why a code failed and lets the person try again", async () => {
    let n = 0;
    const { adapter } = host(async () => {
      if (n++ === 0) throw new HostError("this pairing code was already used", "failed");
      return null;
    });
    await mount(adapter, { initialCode: "K7QM4XPB2RHD" });
    fireEvent.click(join());
    await waitFor(() => expect(state()).toBe("failed"));
    expect(document.querySelector("[data-join-error]")?.textContent).toBe(
      "That code didn't work. It may be used, expired or mistyped. Make a new one in Add a machine.",
    );
    expect(field().disabled).toBe(false);
    fireEvent.click(join());
    await waitFor(() => expect(state()).toBe("joined"));
  });

  it("hides the command's internal prefixes", async () => {
    const { adapter } = host(async () => {
      throw new HostError("cua: unauthenticated: host setup failed at register: relay: pairing code is not valid; make a new one", "failed");
    });
    await mount(adapter, { initialCode: "K7QM4XPB2RHD" });
    fireEvent.click(join());
    await waitFor(() => expect(state()).toBe("failed"));
    const text = document.querySelector("[data-join-error]")?.textContent ?? "";
    expect(text).toContain("That code didn't work");
    expect(text).not.toMatch(/unauthenticated|host setup failed|relay:/);
  });

  it("says plainly when the host can't join with a code", async () => {
    const { adapter } = host(async () => {
      throw new HostError("no such method", "unsupported");
    });
    await mount(adapter, { initialCode: "K7QM4XPB2RHD" });
    fireEvent.click(join());
    await waitFor(() => expect(state()).toBe("failed"));
    expect(document.querySelector("[data-join-error]")?.textContent).toContain("Update the app");
  });
});
