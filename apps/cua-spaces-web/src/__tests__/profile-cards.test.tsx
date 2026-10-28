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

  it("an agent's card says the owner as a person, not an id; clicking selects them in In this Space and opens their card", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false });
    await ready();
    await click(await found("[data-profile-trigger='claude']"));
    const card = await found("[data-profile-card='agent'][data-profile='claude']");
    await click(card.querySelector("[data-card-details]")!);
    const owner = card.querySelector<HTMLElement>("[data-card-owner]")!;
    expect(owner.textContent).toContain("Owner");
    expect(owner.textContent).toContain("Grace");
    expect(card.textContent).not.toContain("acct_grace");
    await click(card.querySelector("[data-card-owner-open]")!);
    // Grace is in the Space: her row is selected and her card is open there.
    await found("[data-profile-card='person'][data-profile='Grace']");
    expect(q("[data-members] [data-profile-trigger='Grace']")?.hasAttribute("data-selected")).toBe(true);
    expect(q("[data-profile-card='agent']")).toBeNull();
  });

  it("an owner who is not in the Space opens their profile card in place, with a way back", async () => {
    await mountApp(path, { failsCreates: false, listsGhost: false, demo: { threadStranger: true } });
    await ready();
    await click(await found("[data-profile-trigger='gemini']"));
    const card = await found("[data-profile-card='agent'][data-profile='gemini']");
    await click(card.querySelector("[data-card-details]")!);
    await click(card.querySelector("[data-card-owner-open]")!);
    const person = await found("[data-profile-card='person'][data-profile='Dana']");
    expect(person.textContent).toContain("Dana");
    // Still in the same popup; Back returns to the agent.
    expect(q("[data-profile-card-owner]")).not.toBeNull();
    await click(q("[data-card-back]")!);
    await found("[data-profile-card='agent'][data-profile='gemini']");
  });

  it("default permissions are one line with an Edit link; Edit shows the three selects, each saved at once", async () => {
    const calls: { op: string; args: unknown }[] = [];
    // Ada's Hermes in release-checks has the default rights.
    await mountApp(`/spaces/${encodeURIComponent("relay:mac-mini/release-checks")}`, { failsCreates: false, listsGhost: false, intercept: (op, args) => (calls.push({ op, args }), undefined) });
    await ready();
    await click(await found("[data-profile-trigger='hermes']"));
    const card = await found("[data-profile-card='agent'][data-profile='hermes']");
    await click(card.querySelector("[data-card-details]")!);
    const line = card.querySelector("[data-card-default-permissions]")!;
    expect(line.textContent).toContain("Default permissions");
    expect(card.querySelectorAll("[data-agent-right]")).toHaveLength(0);
    await click(card.querySelector("[data-card-edit-rights]")!);
    expect(card.querySelectorAll("[data-agent-right]")).toHaveLength(3);
    // Who can give it tasks: Owner, Admins, Members and each person in the Space.
    await click(card.querySelector("[data-agent-right='task'] button")!);
    const options = [...document.querySelectorAll("[role=option]")].map((o) => o.textContent);
    expect(options).toEqual(expect.arrayContaining(["Owner", "Admins", "Members"]));
    expect(options.some((o) => o?.startsWith("Only "))).toBe(true);
    const owner = [...document.querySelectorAll("[role=option]")].find((o) => o.textContent === "Owner")!;
    await act(async () => {
      for (const type of ["pointerdown", "mousedown", "pointerup", "mouseup", "click"]) fireEvent(owner, new MouseEvent(type, { bubbles: true, cancelable: true }));
      await new Promise((r) => setTimeout(r, 20));
    });
    await waitFor(() => expect(calls.some((c) => c.op === "thread.setRights" && (c.args as { name: string; task: string }).task === "owner")).toBe(true));
    // Saved: it is no longer the defaults, so the three rows stay.
    await waitFor(() => expect(card.querySelector("[data-card-default-permissions]")).toBeNull());
    expect(card.querySelectorAll("[data-agent-right]")).toHaveLength(3);
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
