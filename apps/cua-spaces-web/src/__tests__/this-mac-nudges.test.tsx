// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The two This Mac prompts on the real app and the wasm core: a second
// agent on this Mac's desktop (from the access log `host.status` carries),
// and the risky-action question `cua mcp` holds (`nudges.pending`). The
// moments, rules and words are the core's `nudges`.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import type { HostAccess } from "@/bridge/contracts/host";
import { localDay, resetNudgeMemory, resetSignInNudge } from "@/bridge/nudges";
import type { RiskyRequest } from "@/bridge/ops/nudges";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(() => {
  cleanup();
  localStorage.clear();
  resetNudgeMemory("This Mac");
  resetSignInNudge();
});
stubBrowserApis();

const agent = (name: string, id: string) => `ada@example.com (acct-ada) · agent ${name} (${id})`;

function twoAgents(now = Date.now()): HostAccess[] {
  return [
    { atMs: now - 5_000, via: "relay", who: agent("Codex", "agent:codex-5120"), what: "DriverService" },
    { atMs: now - 20_000, via: "relay", who: agent("Claude Code", "agent:claude-code-4242"), what: "ComputerService" },
    { atMs: now - 12 * 60_000, via: "relay", who: agent("Claude Code", "agent:claude-code-4242"), what: "DriverService" },
  ];
}

interface Calls {
  creates: { os?: string }[];
  answers: { id: string; choice: string }[];
}

function host(calls: Calls, opts: { access?: HostAccess[]; pending?: RiskyRequest[] } = {}): TestHost {
  let pending = opts.pending ?? [];
  return {
    failsCreates: false,
    listsGhost: false,
    answer: (op, out) => (op === "host.status" && opts.access ? { ...(out as object), recentAccess: opts.access } : out),
    intercept: (op, args) => {
      if (op === "spaces.create") calls.creates.push(args as { os?: string });
      if (op === "nudges.pending") return pending;
      if (op === "nudges.answer") {
        const a = args as { id: string; choice: string };
        calls.answers.push(a);
        pending = pending.filter((r) => r.id !== a.id);
        return null;
      }
      return undefined;
    },
  };
}

const card = (kind: string) =>
  waitFor(() => {
    const el = document.querySelector(`[data-nudge=${kind}]`);
    expect(el).toBeTruthy();
    return el as HTMLElement;
  });

const settle = () => act(() => new Promise((r) => setTimeout(r, 60)));

describe.skipIf(!wasmBuilt)("This Mac prompts (real routes, wasm core)", () => {
  it("a second agent on the desktop: shared mouse, Add a Space starts macOS in the background", async () => {
    const calls: Calls = { creates: [], answers: [] };
    const { router } = await mountApp("/spaces", host(calls, { access: twoAgents() }));
    await router.load();
    const el = await card("second-agent");
    expect(el.textContent).toContain("Codex just started on this Mac");
    expect(el.textContent).toContain("Claude Code is already working here.");
    expect(el.textContent).toContain("They'll share your mouse. Add a Space so they each get their own?");
    expect(el.querySelector("[data-nudge-secondary]")?.textContent).toBe("Keep sharing");
    fireEvent.click(el.querySelector("[data-nudge-primary]")!);
    expect(document.querySelector("[data-nudge]")).toBeNull();
    await waitFor(() => expect(calls.creates.length).toBe(1), { timeout: 4_000 });
    expect(calls.creates[0]?.os).toBe("macos");
  });

  it("once a day per computer, and never after Don't ask again", async () => {
    const calls: Calls = { creates: [], answers: [] };
    const first = await mountApp("/spaces", host(calls, { access: twoAgents() }));
    await first.router.load();
    fireEvent.click((await card("second-agent")).querySelector("[data-nudge-secondary]")!);
    expect(JSON.parse(localStorage.getItem("cua.nudge.secondAgent.This Mac")!).shownDay).toBe(localDay());
    cleanup();
    // Same day: not again.
    const again = await mountApp("/spaces", host(calls, { access: twoAgents() }));
    await again.router.load();
    await settle();
    expect(document.querySelector("[data-nudge]")).toBeNull();
    cleanup();
    // Another day it may show; Don't ask again turns it off for good.
    localStorage.setItem("cua.nudge.secondAgent.This Mac", JSON.stringify({ shownDay: "2000-01-01", off: false }));
    const later = await mountApp("/spaces", host(calls, { access: twoAgents() }));
    await later.router.load();
    fireEvent.click((await card("second-agent")).querySelector("[data-nudge-never]")!);
    expect(JSON.parse(localStorage.getItem("cua.nudge.secondAgent.This Mac")!).off).toBe(true);
    expect(calls.creates).toEqual([]);
  });

  it("no prompt for one agent", async () => {
    const calls: Calls = { creates: [], answers: [] };
    const { router } = await mountApp("/spaces", host(calls, { access: twoAgents().slice(1) }));
    await router.load();
    await settle();
    expect(document.querySelector("[data-nudge]")).toBeNull();
  });

  it("a risky action: Run it here answers here; a separate Space answers separate and starts one", async () => {
    const calls: Calls = { creates: [], answers: [] };
    const now = Date.now();
    const pending: RiskyRequest[] = [
      { id: "risky-4242-1", agent: "Claude Code", action: "run an installer", atMs: now, expiresMs: now + 30_000 },
      { id: "risky-4242-2", agent: "Claude Code", action: "empty the Trash", atMs: now + 1, expiresMs: now + 30_000 },
    ];
    const { router } = await mountApp("/spaces", host(calls, { pending }));
    await router.load();
    const el = await card("risky-action");
    expect(el.textContent).toContain("Claude Code wants to run an installer on your desktop");
    expect(el.textContent).toContain("Run this in a separate Space instead?");
    expect(el.textContent).toMatch(/If you don't choose, it runs here in \d+ seconds\./);
    expect(el.querySelector("[data-nudge-never]")).toBeNull();
    fireEvent.click(el.querySelector("[data-nudge-secondary]")!);
    expect(calls.answers).toEqual([{ id: "risky-4242-1", choice: "here" }]);
    const next = await waitFor(
      () => {
        const n = document.querySelector("[data-nudge=risky-action]");
        expect(n?.textContent).toContain("empty the Trash");
        return n as HTMLElement;
      },
      { timeout: 3_000 },
    );
    fireEvent.click(next.querySelector("[data-nudge-primary]")!);
    expect(calls.answers[1]).toEqual({ id: "risky-4242-2", choice: "separate" });
    await waitFor(() => expect(calls.creates.length).toBe(1), { timeout: 4_000 });
  });

  it("signed out with a Space ready: asks once to sign in; Sign in starts the browser sign-in (CUA-2062)", async () => {
    const signIns: unknown[] = [];
    const tracked: string[] = [];
    const signedOut = (): TestHost => ({
      failsCreates: false,
      listsGhost: false,
      answer: (op, out) => {
        if (op !== "session.get") return out;
        const o = out as { fleet: Record<string, unknown> };
        return { ...o, fleet: { ...o.fleet, configured: false, authMode: "none", identity: undefined, namespaces: [] } };
      },
      intercept: (op, args) => {
        if (op === "session.signIn") {
          signIns.push(args);
          return { method: "browser", verificationUri: "https://cua.ai/signin" };
        }
        if (op === "telemetry.track") {
          for (const s of (args as { signals: { type: string; step?: string }[] }).signals) if (s.type === "step" && s.step?.startsWith("signin_prompt")) tracked.push(s.step);
          return null;
        }
        return undefined;
      },
    });
    const first = await mountApp("/spaces", signedOut());
    await first.router.load();
    const el = await card("sign-in");
    expect(el.textContent).toContain("Sign in to share this Space with your team.");
    expect(el.querySelector("[data-nudge-secondary]")?.textContent).toBe("Not now");
    fireEvent.click(el.querySelector("[data-nudge-primary]")!);
    expect(document.querySelector("[data-nudge=sign-in]")).toBeNull();
    await waitFor(() => expect(signIns.length).toBe(1));
    await waitFor(() => expect(tracked).toEqual(["signin_prompt_shown", "signin_prompt_accepted"]));
    cleanup();
    // Once per computer.
    const again = await mountApp("/spaces", signedOut());
    await again.router.load();
    await settle();
    expect(document.querySelector("[data-nudge=sign-in]")).toBeNull();
  });

  it("no sign-in prompt when signed in", async () => {
    const calls: Calls = { creates: [], answers: [] };
    const { router } = await mountApp("/spaces", host(calls));
    await router.load();
    await settle();
    expect(document.querySelector("[data-nudge=sign-in]")).toBeNull();
  });
});
