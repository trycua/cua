// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A machine its owner stopped sharing is not counted
// online and reads "Not reachable" (the core's word, as the SwiftUI detail
// shows it), its Status line wraps instead of being cut, and it can be
// removed with the core's Delete action, as the SwiftUI toolbar has it.

import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import type { MachineRow } from "@/bridge/contracts/host";
import { withNotSharing } from "@/bridge/sharing";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const LINE = "Studio stopped sharing: ask its owner to Resume sharing (or run `cua host start` there)";

function host(removed: string[]): TestHost {
  return {
    failsCreates: false,
    listsGhost: false,
    answer: (op, out) => {
      if (op === "spaces.list") {
        if (removed.includes("relay:studio")) return out;
        return [...(out as unknown[]), { id: "relay:studio", name: "Studio", provider: "relay", spacesdVersion: "0.6.0", features: [], reachable: false, error: LINE }];
      }
      if (op === "machines.list") {
        if (removed.includes("relay:studio")) return out;
        const rows = out as MachineRow[];
        return [...rows, { id: "studio", name: "Studio", via: "relay", online: true, os: "", detail: LINE, limits: withNotSharing([], LINE) }];
      }
      return out;
    },
    intercept: (op, args) => {
      if (op !== "spaces.delete") return undefined;
      removed.push((args as { spaceId: string }).spaceId);
      return "";
    },
  };
}

describe.skipIf(!wasmBuilt)("a machine its owner stopped sharing (real routes, wasm core)", () => {
  it("is not counted online, reads Not reachable, shows its whole Status line, and can be removed", async () => {
    const removed: string[] = [];
    const { router, hooks } = await mountApp("/machines", host(removed));
    await router.load();
    await waitFor(() => expect(hooks.machines.data?.some((m) => m.id === "studio")).toBe(true));
    const total = hooks.machines.data!.length;
    const online = hooks.machines.data!.filter((m) => m.online && !m.notSharing).length;
    await screen.findByText(`${total} machines, ${online} online`);

    const row = document.querySelector('[data-machine-id="studio"]') as HTMLElement;
    expect(within(row).getByText("Not reachable")).toBeTruthy();
    act(() => row.click());
    const detail = await screen.findByRole("region", { name: "Studio details" });
    expect(within(detail).queryByText(/Online, not sharing/)).toBeNull();
    expect(within(detail).getAllByText("Not reachable").length).toBeGreaterThan(0);
    const status = within(detail).getByText("Status").closest("div")!.parentElement!.parentElement!;
    const value = status.querySelector("[data-fact-value]")!;
    expect(value.textContent).toContain("cua host start");
    expect(value.className).not.toContain("truncate");

    const remove = detail.querySelector('[data-machine-remove="studio"]') as HTMLButtonElement;
    expect(remove).not.toBeNull();
    fireEvent.click(remove);
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getAllByRole("button").at(-1)!);
    await waitFor(() => expect(removed).toEqual(["relay:studio"]));
  });
});
