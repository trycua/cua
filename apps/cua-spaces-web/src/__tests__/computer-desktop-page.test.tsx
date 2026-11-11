// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A computer's row opens its desktop's Space page, which also manages the computer: Computer settings in the header
// menu, and a trash: the computer you are on offers Stop sharing and Stop sharing and sign out (only Sign out once it is
// not shared); another computer is one confirmation. Stopping sharing is also the page's own button.

import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { removePrompt } from "@/components/computers/computer-actions";
import { resetCollapsed } from "@/components/spaces/tree-parts";
import { resetAskTasks } from "@/lib/ask-first";
import { resetPrepare } from "@/lib/prepare/store";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
  resetPrepare();
  resetCollapsed();
});
afterEach(() => {
  cleanup();
  resetPrepare();
});

type Call = [string, Record<string, unknown>];
const q = (s: string) => document.querySelector<HTMLElement>(s);
const found = (s: string) =>
  waitFor(() => {
    const el = q(s);
    if (!el) throw new Error(`not yet in the page: ${s}`);
    return el;
  });

const host = (calls: Call[]): TestHost => ({
  failsCreates: false,
  listsGhost: false,
  demo: { onboarded: true, desktops: true },
  intercept: (op, args) => {
    if (op === "machines.remove" || op === "session.signOut") {
      calls.push([op, args as Record<string, unknown>]);
      return null;
    }
    return undefined;
  },
});

describe.skipIf(!wasmBuilt)("a computer's desktop page manages the computer", () => {
  it("has a trash for another computer and Computer settings in its menu; Remove waits while it is online", { timeout: 30000 }, async () => {
    const calls: Call[] = [];
    await mountApp("/spaces/relay%3Alinux-box", host(calls));
    const trash = await found("[data-header-remove-computer]");
    expect(trash.getAttribute("aria-label")).toBe("Remove Linux box");
    // The relay takes a computer off only while it is not connected: shown, and says so.
    expect(trash.getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(trash);
    expect(screen.queryByText("Sign this computer out of your account?")).toBeNull();
    fireEvent.click(await found("[data-header-more]"));
    expect(await screen.findByText("Computer settings")).toBeTruthy();
    expect(calls).toEqual([]);
  });

  it("offers Stop sharing, and Stop sharing and sign out, to remove the computer you are on while it shares", { timeout: 30000 }, async () => {
    const calls: Call[] = [];
    const { router } = await mountApp("/spaces/this-mac", host(calls));
    fireEvent.click(await found("[data-header-remove-computer]"));
    expect(await screen.findByRole("button", { name: "Stop sharing and sign out" })).toBeTruthy();
    expect(screen.getAllByRole("button", { name: "Stop sharing" }).length).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Sign out" })).toBeNull();
    // Cancel leaves everything as it is.
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Stop sharing and sign out" })).toBeNull());
    expect(calls).toEqual([]);
    fireEvent.click(q("[data-header-remove-computer]")!);
    fireEvent.click(await screen.findByRole("button", { name: "Stop sharing and sign out" }));
    await waitFor(() => expect(calls.map((c) => c[0])).toEqual(["session.signOut"]));
    // Signed out, the page leaves for the list.
    await waitFor(() => expect(router.state.location.pathname).toBe("/spaces"));
  });

  it("offers only Sign out once the computer you are on is not shared", { timeout: 30000 }, async () => {
    const calls: Call[] = [];
    await mountApp("/spaces/this-mac", { ...host(calls), demo: { onboarded: true, desktops: true, desktopOff: true } });
    // Stop sharing first, from the first prompt.
    fireEvent.click(await found("[data-header-remove-computer]"));
    fireEvent.click(await screen.findByRole("button", { name: "Stop sharing" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Stop sharing" })).toBeNull());
    await waitFor(() => expect(q("[data-call-header]")?.textContent).toMatch(/Not sharing/));
    fireEvent.click(q("[data-header-remove-computer]")!);
    expect(await screen.findByText("Sign this computer out of your account?")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Stop sharing/ })).toBeNull();
    fireEvent.click(await screen.findByRole("button", { name: "Sign out" }));
    await waitFor(() => expect(calls.map((c) => c[0])).toEqual(["session.signOut"]));
  });

  it("words the prompt for each: one confirmation for another computer", () => {
    expect(removePrompt({ current: false, sharing: true, name: "Mac mini" })).toMatchObject({
      confirm: "Remove",
      alternative: null,
      title: "Sign this computer out of your account?",
    });
    expect(removePrompt({ current: true, sharing: true, name: "This Mac" })).toMatchObject({
      confirm: "Stop sharing",
      alternative: "Stop sharing and sign out",
    });
    expect(removePrompt({ current: true, sharing: false, name: "This Mac" })).toMatchObject({ confirm: "Sign out", alternative: null });
  });
});
