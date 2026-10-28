// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Tips (CUA-2204) on the real routes, the wasm core and the demo host: one
// at a time, chosen by the core from what the page knows; Copy says
// "Copied" in place; Next tip, × and Don't show tips are kept in the app
// settings (`values.tips`); Select stopped opens the list for This Mac;
// powering from a tile and sharing finish their tips; the sidebar's tip
// waits while a page shows one; Settings' Show tips and Reset tips.

import { act, cleanup, fireEvent, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import type { TipsState } from "@/bridge";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const written: string[] = [];
beforeEach(() => {
  written.length = 0;
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: (t: string) => (written.push(t), Promise.resolve()) },
  });
});

const tip = (where: ParentNode = document) => where.querySelector<HTMLElement>("main [data-tip]");
const sidebarTip = () => document.querySelector<HTMLElement>("aside [data-tip]");
const shown = (id: string) => waitFor(() => expect(tip()?.getAttribute("data-tip")).toBe(id));
const button = (el: HTMLElement, name: string) => within(el).getByRole("button", { name });

/** The demo host, with its settings writes recorded. */
function host(extra: Partial<TestHost> = {}) {
  const saved: TipsState[] = [];
  const h: TestHost = {
    failsCreates: false,
    listsGhost: false,
    ...extra,
    answer: (op, out) => {
      const values = (out as { values?: { tips?: TipsState } } | null)?.values;
      if (op === "settings.set" && values?.tips) saved.push(values.tips);
      return extra.answer ? extra.answer(op, out) : out;
    },
  };
  return { h, saved };
}

const settingsTips = async (hooks: Awaited<ReturnType<typeof mountApp>>["demo"]) =>
  ((await hooks.call("settings.get", {})) as { values: { tips?: TipsState } }).values.tips;

describe.skipIf(!wasmBuilt)("tips", () => {
  it("offers the newest running Space in your agent, its request read-only with Copy that says Copied", async () => {
    const { h } = host();
    await mountApp("/spaces", h);
    await shown("try-agent");
    const t = tip()!;
    // Claude Code is the first connected of Hermes, Claude Code, Codex and Cursor on the demo host.
    expect(t.querySelector("[data-tip-text]")!.textContent).toMatch(/^Try .+ in Claude Code$/);
    const code = t.querySelector<HTMLTextAreaElement>("textarea[data-tip-code]")!;
    expect(code.readOnly).toBe(true);
    expect(code.value).toMatch(/^Open the Cua Space ".+" and tell me what's on its screen\.$/);
    // No Open in for Claude Code; the lightbulb, no "Tip" label.
    expect(t.querySelector("[data-tip-open-in]")).toBeNull();
    expect(t.querySelector("[data-tip-mark]")).not.toBeNull();
    expect(t.textContent).not.toMatch(/\bTip\b/);
    fireEvent.click(button(t, "Copy"));
    expect(written).toEqual([code.value]);
    await waitFor(() => expect(within(t).getByRole("button", { name: "Copied" })).toBeTruthy());
  });

  it("Next tip skips to the next that applies, and × closes it; both are kept in the settings", async () => {
    const { h, saved } = host();
    const app = await mountApp("/spaces", h);
    await shown("try-agent");
    fireEvent.click(button(tip()!, "Next tip"));
    // The demo has a stopped Space: power on hover is next.
    await shown("power");
    expect(tip()!.textContent).toContain("Point at a Space to turn it on or off without opening it.");
    expect(saved.at(-1)).toEqual({ off: false, done: ["try-agent"] });
    fireEvent.click(button(tip()!, "Close"));
    await waitFor(() => expect(tip()).toBeNull());
    expect(await settingsTips(app.demo)).toEqual({ off: false, done: ["try-agent", "power"] });
  });

  it("Don't show tips turns them all off, the sidebar's too", async () => {
    const { h } = host();
    const app = await mountApp("/spaces", h);
    await shown("try-agent");
    fireEvent.click(button(tip()!, "Don't show tips"));
    await waitFor(() => expect(tip()).toBeNull());
    expect((await settingsTips(app.demo))?.off).toBe(true);
    await act(() => app.router.navigate({ to: "/machines" }));
    await waitFor(() => expect(document.querySelector("[data-machines-page], main")).not.toBeNull());
    expect(sidebarTip()).toBeNull();
  });

  it("shows a tip that belongs to no page in the sidebar, only while no page shows one", async () => {
    const { h } = host();
    const app = await mountApp("/spaces", h);
    await shown("try-agent");
    // Two computers are online, but the page's tip comes first.
    expect(sidebarTip()).toBeNull();
    await act(() => app.router.navigate({ to: "/machines" }));
    await waitFor(() => expect(sidebarTip()?.getAttribute("data-tip")).toBe("on-auto"));
    const s = sidebarTip()!;
    expect(s.textContent).toContain("New Spaces can go to whichever machine has room.");
    expect(s.querySelector("[data-tip-code]")!.textContent).toBe("cua create --on auto");
    // Only × in the sidebar: no Next tip, no Don't show tips.
    expect(within(s).queryByRole("button", { name: "Next tip" })).toBeNull();
    fireEvent.click(within(s).getByRole("button", { name: "Copy" }));
    expect(written).toEqual(["cua create --on auto"]);
    await waitFor(() => expect(within(s).getByRole("button", { name: "Copied" })).toBeTruthy());
    fireEvent.click(within(s).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(sidebarTip()).toBeNull());
    expect((await settingsTips(app.demo))?.done).toContain("on-auto");
  });

  it("several stopped Spaces and a nearly full disk: Select stopped opens the list for This Mac, biggest first, the stopped ones picked", async () => {
    const { h } = host({ demo: { disk: "low" } });
    const app = await mountApp("/spaces", h);
    await shown("try-agent");
    await app.demo.call("settings.set", { key: "tips", value: { off: false, done: ["try-agent", "power"] } });
    app.emit({ type: "settings.changed" });
    // The demo's stopped Spaces you may delete: fewer than 3 gives no tip, so make sure of 3.
    await shown("bulk-select");
    expect(tip()!.textContent).toContain("Select several Spaces to delete them together.");
    fireEvent.click(button(tip()!, "Select stopped"));
    await waitFor(() => expect(app.router.state.location.search).toEqual({ view: "list", on: "this-mac", sort: "size", select: "stopped" }));
  });

  it("powering a Space from its tile finishes the power tip", async () => {
    const { h } = host();
    const app = await mountApp("/spaces", h);
    await shown("try-agent");
    const power = await waitFor(() => {
      const b = document.querySelector<HTMLButtonElement>("[data-tile-power]");
      if (!b) throw new Error("no tile power");
      return b;
    });
    fireEvent.click(power);
    await waitFor(async () => expect((await settingsTips(app.demo))?.done).toContain("power"));
  });

  it("on a Space's page: its request first, then Share… (sharing any Space finishes it)", async () => {
    const { h } = host({
      answer: (op, out) =>
        op === "spaces.list"
          ? (out as Array<Record<string, unknown>>).map((space) => ({ ...space, sharing: undefined }))
          : out,
    });
    const app = await mountApp("/spaces/local:design-review", h);
    await shown("try-agent");
    expect(tip()!.querySelector("[data-tip-text]")!.textContent).toBe("Try design-review in Claude Code");
    fireEvent.click(button(tip()!, "Next tip"));
    await shown("share");
    expect(tip()!.textContent).toContain("Share design-review with a teammate.");
    fireEvent.click(button(tip()!, "Share…"));
    // The Share sheet opens, as the header's Share does.
    await waitFor(() => expect(document.querySelector("[role=dialog]")).not.toBeNull());
    void app;
  });

  it("names Cursor and opens the request in it", async () => {
    const opened: string[] = [];
    const { h } = host({
      intercept: (op, args) => {
        if (op === "session.openExternal") return (opened.push((args as { url: string }).url), null);
        return undefined;
      },
      answer: (op, out) =>
        op === "agents.setup" ? (out as { agent: string; configured: boolean }[]).map((r) => ({ ...r, configured: r.agent === "cursor" })) : out,
    });
    await mountApp("/spaces", h);
    await shown("try-agent");
    const t = tip()!;
    expect(t.querySelector("[data-tip-text]")!.textContent).toMatch(/ in Cursor$/);
    fireEvent.click(button(t, "Open in Cursor"));
    await waitFor(() => expect(opened[0]).toMatch(/^cursor:\/\/anysphere\.cursor-deeplink\/prompt\?text=Open%20the%20Cua%20Space/));
  });

  it("keeps them for the session on a host that keeps no core settings", async () => {
    const { h } = host({
      intercept: (op, args) => {
        if (op === "settings.set" && (args as { key: string }).key === "tips") throw new Error("unsupported");
        return undefined;
      },
    });
    await mountApp("/spaces", h);
    await shown("try-agent");
    fireEvent.click(button(tip()!, "Next tip"));
    await shown("power");
  });
});

describe.skipIf(!wasmBuilt)("Settings, General: Show tips and Reset tips", () => {
  const row = (label: string) =>
    waitFor(() => {
      const el = document.querySelector<HTMLElement>(`main [data-setting-row="${label === "Show tips" ? "tips" : "reset-tips"}"]`);
      if (!el) throw new Error(`no ${label} row`);
      return el;
    });

  it("turns tips off and on (the ones not done come back), and Reset starts them over", async () => {
    const { h } = host();
    const app = await mountApp("/settings/general", h);
    await app.demo.call("settings.set", { key: "tips", value: { off: false, done: ["try-agent"] } });
    app.emit({ type: "settings.changed" });
    const show = await row("Show tips");
    const toggle = within(show).getByRole("switch");
    await waitFor(() => expect(toggle.getAttribute("aria-checked")).toBe("true"));
    fireEvent.click(toggle);
    await waitFor(async () => expect(await settingsTips(app.demo)).toEqual({ off: true, done: ["try-agent"] }));
    fireEvent.click(within(await row("Show tips")).getByRole("switch"));
    await waitFor(async () => expect(await settingsTips(app.demo)).toEqual({ off: false, done: ["try-agent"] }));
    fireEvent.click(within(await row("Reset tips")).getByRole("button", { name: "Reset" }));
    await waitFor(async () => expect(await settingsTips(app.demo)).toEqual({ off: false, done: [] }));
  });
});

void vi;
