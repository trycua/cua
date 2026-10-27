// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The cards people and agents open to on the redesigned Space page: a
// person's (email, access, Active now; Change access and Remove for the
// owner), your own, and an agent of yours with no sign-in (Not
// connected, with Connect). Mentions open the same cards. With the redesign
// off, the page as it was has none.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { DEMO_THREAD_SPACE } from "@/bridge/adapters/demo/thread";
import { resetConnectState } from "@/hooks/use-connectors";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetConnectState();
});
afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
});

const path = `/spaces/${encodeURIComponent(DEMO_THREAD_SPACE)}`;
const q = (s: string) => document.querySelector<HTMLElement>(s);
const found = (s: string) =>
  waitFor(() => {
    const el = q(s);
    expect(el).toBeTruthy();
    return el!;
  });
/** The page has settled (the thread read, the composer on): its triggers stay put. */
const ready = () => waitFor(() => expect((q("[data-composer]") as HTMLTextAreaElement | null)?.disabled).toBe(false));
const click = (el: Element) =>
  act(async () => {
    fireEvent.click(el);
    await new Promise((r) => setTimeout(r, 10));
  });

describe.skipIf(!wasmBuilt)("profile cards (redesign)", () => {
  it("the owner opens an editor's card: email, Can edit, Active now, Change access, Remove; no Copy email", async () => {
    const calls: { op: string; args: unknown }[] = [];
    await mountApp(path, { failsCreates: false, listsGhost: false, intercept: (op, args) => (calls.push({ op, args }), undefined) });
    await ready();
    await click(await found("[data-profile-trigger='Grace']"));
    const card = await found("[data-profile-card='person'][data-profile='Grace']");
    expect(card.textContent).toContain("grace@example.com");
    expect(card.querySelector("[data-card-access]")?.textContent).toContain("Can edit");
    expect(card.querySelector("[data-card-presence]")?.textContent).toBe("Active now");
    expect(card.textContent).not.toContain("Copy email");
    const remove = card.querySelector<HTMLElement>("[data-card-remove]")!;
    expect(remove.textContent).toBe("Remove from Space");
    await click(remove);
    await waitFor(() => expect(calls.some((c) => c.op === "sharing.unshare" && (c.args as { who: string }).who === "grace@example.com")).toBe(true));
  });

  it("your own card: (you), Owner, no access actions", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    await ready();
    await click(await found("[data-profile-trigger='Ada']"));
    const card = await found("[data-profile-card='person'][data-profile='Ada']");
    expect(card.textContent).toContain("(you)");
    expect(card.querySelector("[data-card-access]")?.textContent).toContain("Owner");
    expect(card.querySelector("[data-card-remove]")).toBeNull();
    expect(card.querySelector("[role=combobox]")).toBeNull();
  });

  it("your agent with no sign-in it can use: Not connected, with Connect", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false, answer: (op, out) => (op === "agentKeys.list" ? { ...(out as object), keys: [] } : out) });
    await ready();
    await click(await found("[data-profile-trigger='hermes']"));
    const card = await found("[data-profile-card='agent'][data-profile='hermes']");
    await waitFor(() => expect(card.querySelector("[data-card-status]")?.getAttribute("data-card-status")).toBe("not-connected"));
    expect(card.querySelector("[data-card-status]")?.textContent).toBe("Not connected");
    expect(card.querySelector("[data-card-connect]")?.textContent).toBe("Connect Hermes");
    expect(card.querySelector("[data-card-line='who']")?.textContent).toBe("Run by you in this Space, uses Anthropic");
  });

  it("a mention chip opens its member's card", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    await ready();
    const chip = await found("[data-profile-trigger] [data-mention='hermes']");
    await click(chip.closest("[data-profile-trigger]")!);
    await found("[data-profile-card='agent'][data-profile='hermes']");
  });

  it("redesign off: the page as it was, no cards", async () => {
    vi.stubEnv("VITE_CUA_FLAG_SPACES_REDESIGN", "0");
    await mountApp(path, { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(document.body.textContent).toContain("ubuntu-build"));
    expect(q("[data-profile-trigger]")).toBeNull();
  });
});
