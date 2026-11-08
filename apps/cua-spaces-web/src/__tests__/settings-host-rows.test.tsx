// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, Advanced on a host with settings of its own: one row per kind
// of Space (Containers, Virtual machines, MicroVMs) as the core lays them
// out, "Already installed" with what is installed, the path in use and a
// live check of a typed one; "Open a Space's screen right away". Rendered
// from the demo host and from the SwiftUI host's own `settings.get`
// answers (fixtures the Mac app's tests write), through the WebKit
// adapter's conversion, as the real app draws them.

import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { BridgeProvider } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { toSettingsSnapshot } from "@/bridge/adapters/webkit";
import type { WkSettings } from "@/bridge/webkit-protocol";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { ToastProvider } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { routeTree } from "../routeTree.gen";
import { stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

/** Settings, Advanced on `adapter` (the demo host by default), with what it was asked. */
async function mountAdvanced(wrap?: (demo: DataAdapter) => DataAdapter) {
  const demo = createDemoAdapter({ latencyMs: 1, stepMs: 4 });
  const calls: [string, unknown][] = [];
  const base = wrap ? wrap(demo) : demo;
  const adapter: DataAdapter = {
    mode: base.mode,
    subscribe: (l) => base.subscribe(l),
    call: (async (op: string, args: unknown) => {
      calls.push([op, args]);
      return base.call(op as never, args as never);
    }) as DataAdapter["call"],
  };
  const router = createRouter({ routeTree, history: createMemoryHistory({ initialEntries: ["/settings/advanced"] }) });
  render(
    <BridgeProvider adapter={adapter} core={await testCore()} storeOptions={{ tickMs: 5 }}>
      <TooltipProvider delay={500}>
        <ToastProvider>
          <RouterProvider router={router} />
        </ToastProvider>
      </TooltipProvider>
    </BridgeProvider>,
  );
  const chosen = () => calls.filter(([op]) => op === "settings.choose").map(([, a]) => a as { row: string; option: string });
  return { calls, chosen };
}

const row = (id: string) => document.querySelector<HTMLElement>(`[data-setting-row="${id}"]`);
/** The row once it is there. */
const rowSoon = (id: string) =>
  waitFor(() => {
    const r = row(id);
    if (!r) throw new Error(`no ${id} row`);
    return r;
  });
const kindRow = (kind: string) => document.querySelector<HTMLElement>(`[data-runtime-kind="${kind}"]`);
const KINDS: [string, string][] = [
  ["container", "Containers"],
  ["vm", "Virtual machines"],
  ["microvm", "MicroVMs"],
];

async function pick(scope: HTMLElement, name: string) {
  const trigger = within(scope).getByRole("combobox");
  trigger.focus();
  fireEvent.keyDown(trigger, { key: "ArrowDown" });
  const option = await screen.findByRole("option", { name });
  fireEvent.pointerMove(option, { pointerType: "mouse" });
  fireEvent.mouseMove(option);
  fireEvent.keyDown(option, { key: "Enter" });
  fireEvent.keyUp(option, { key: "Enter" });
}

/** The SwiftUI host's `settings.get` answer, as its tests wrote it. */
const macHostAnswer = (name: string): WkSettings =>
  JSON.parse(readFileSync(join(__dirname, "fixtures", `mac-host-settings-${name}.json`), "utf8")) as WkSettings;

/** The demo host, but `settings.get` answers what the SwiftUI host answered, through the WebKit adapter. */
const asMacHost = (name: string) => (demo: DataAdapter): DataAdapter => ({
  ...demo,
  call: (async (op: string, args: unknown) => {
    if (op === "settings.get") return toSettingsSnapshot(macHostAnswer(name), { theme: "system", hotkey: "⌘⇧Space" });
    return demo.call(op as never, args as never);
  }) as DataAdapter["call"],
});

describe.skipIf(!wasmBuilt)("Settings, Advanced: which software runs Spaces (real routes, wasm core)", () => {
  it("has one row per kind, in plain words, and none of the old per-OS rows", async () => {
    await mountAdvanced();
    await waitFor(() => expect(kindRow("microvm")).toBeTruthy());
    expect(document.body.textContent).toContain("Which software runs Spaces on this computer");
    for (const [kind, label] of KINDS) {
      const r = row(`runtime:${kind}`)!;
      expect(within(r).getByText(label)).toBeTruthy();
    }
    expect(row("runtime:container")!.textContent).toContain("Automatic");
    // The demo Mac (macOS 15) can't run microVMs: it says so, and why.
    expect(row("runtime:microvm")!.textContent).toContain("Not available");
    expect(row("runtime:microvm")!.textContent).toContain("MicroVMs need macOS 26");
    expect(row("macos-runtime")).toBeNull();
    expect(row("linux-runtime")).toBeNull();
    expect(document.body.textContent).not.toMatch(/Mac computers|Linux computers|Built-in|gVisor/);
    // Automatic shows nothing more.
    expect(document.querySelector("[data-runtime-installed]")).toBeNull();
  });

  it("Already installed lists what is installed, autofills the path in use and checks a typed one", async () => {
    const { chosen } = await mountAdvanced();
    const containers = await rowSoon("runtime:container");
    await pick(containers, "Already installed");
    await waitFor(() => expect(chosen()).toContainEqual({ row: "runtime:container", option: "system" }));
    const path = await rowSoon("runtime-path:container");
    const field = within(path).getByRole("textbox") as HTMLInputElement;
    // Autofilled with the binary that will be used, Detected, and what it is.
    expect(field.value).toBe("/usr/local/bin/docker");
    expect(path.dataset.runtimeState).toBe("detected");
    await waitFor(() => expect(path.querySelector("[data-runtime-check]")?.textContent).toBe("Docker 27.3 found"));
    // The drop-down names what is installed.
    const detected = row("runtime-detected:container")!;
    expect(detected.textContent).toContain("Docker — /usr/local/bin/docker (27.3)");

    // Something that is not Docker: checked while typed, not saved.
    fireEvent.change(field, { target: { value: "/bin/ls" } });
    await waitFor(() => expect(path.querySelector("[data-runtime-check='bad']")?.textContent).toBe("Not a Docker or Podman binary"));
    expect(path.dataset.runtimeState).toBe("custom");
    expect(chosen().some((c) => c.row === "runtime-path:container")).toBe(false);

    // Another Docker: saved on Enter, Custom, with Reset to detected.
    fireEvent.change(field, { target: { value: "/Users/demo/bin/docker" } });
    await waitFor(() => expect(path.querySelector("[data-runtime-check='ok']")?.textContent).toBe("Docker 28.0 found"));
    fireEvent.keyDown(field, { key: "Enter" });
    await waitFor(() => expect(chosen()).toContainEqual({ row: "runtime-path:container", option: "/Users/demo/bin/docker" }));
    await waitFor(() => expect(row("runtime-path:container")!.dataset.runtimeState).toBe("custom"));
    const reset = await waitFor(() => within(row("runtime-path:container")!).getByText("Reset to detected"));
    fireEvent.click(reset);
    await waitFor(() => expect(chosen()).toContainEqual({ row: "runtime-path:container", option: "" }));
    await waitFor(() => expect((within(row("runtime-path:container")!).getByRole("textbox") as HTMLInputElement).value).toBe("/usr/local/bin/docker"));
  });

  it("an older Lume is named too old", async () => {
    const { chosen } = await mountAdvanced();
    await pick(await rowSoon("runtime:vm"), "Already installed");
    const path = await rowSoon("runtime-path:vm");
    fireEvent.click(within(path).getByText("Browse…"));
    await waitFor(() => expect(chosen()).toContainEqual({ row: "runtime-path:vm", option: "/opt/homebrew/Cellar/lume/0.5.3/bin/lume" }));
    await waitFor(() => expect(row("runtime-path:vm")!.querySelector("[data-runtime-check='bad']")?.textContent).toBe("Lume 0.5.3 is too old (needs 0.6.0)"));
  });

  for (const answer of ["unread", "read"]) {
    it(`draws all three kinds from the Mac app's own settings.get (${answer})`, async () => {
      await mountAdvanced(asMacHost(answer));
      await waitFor(() => expect(kindRow("microvm")).toBeTruthy());
      for (const [kind, label] of KINDS) {
        expect(kindRow(kind), kind).toBeTruthy();
        expect(within(row(`runtime:${kind}`)!).getByText(label)).toBeTruthy();
      }
    });
  }
});
