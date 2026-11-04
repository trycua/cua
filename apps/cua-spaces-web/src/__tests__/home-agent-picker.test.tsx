// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// CUA-3193: Home's "Who does it" offers only the agents on this computer. With none, it says
// "Install Claude Code" instead of @claude, Start stays off, and the new setup prepares no agent.

import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { setFlagOverride } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetConnectState } from "@/hooks/use-connectors";
import { resetAskTasks } from "@/lib/ask-first";
import { resetPrepare } from "@/lib/prepare/store";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
  resetConnectState();
  resetPrepare();
  setFlagOverride("spaces_ask_first_home", true);
});
afterEach(() => {
  cleanup();
  resetPrepare();
  vi.unstubAllEnvs();
});

const q = (s: string) => document.querySelector<HTMLElement>(s);
type Call = { op: string; args: Record<string, unknown> };

describe.skipIf(!wasmBuilt)("Home's agent picker", () => {
  it("with no agent on this computer: Install Claude Code, no @claude, Start off, nothing prepared", { timeout: 30000 }, async () => {
    setFlagOverride("spaces_new_onboarding", true);
    const calls: Call[] = [];
    await mountApp("/home", { failsCreates: false, listsGhost: false, demo: { onboarded: true, agents: "none" }, intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined) });
    await waitFor(() => expect(calls.some((c) => c.op === "agents.setup")).toBe(true));
    await waitFor(() => expect(q("[data-ask-install]")?.textContent).toBe("Install Claude Code"));
    expect(q("[data-ask-mention]")).toBeNull();
    expect(q('[data-picker="who"]')?.textContent).not.toContain("@");
    fireEvent.change(await screen.findByLabelText("What do you want to do?"), { target: { value: "fix the build" } });
    expect((screen.getByRole("button", { name: "Start" }) as HTMLButtonElement).disabled).toBe(true);
    // The new setup does not make a home for an agent that isn't there.
    await new Promise((r) => setTimeout(r, 500));
    expect(calls.some((c) => c.op === "thread.agentAdd")).toBe(false);
    expect(calls.some((c) => c.op === "spaces.create" && (c.args.config as { name?: string } | undefined)?.name === "home-claude-code")).toBe(false);
  });

  it("with only Codex on this computer: Codex, never Claude Code", { timeout: 30000 }, async () => {
    await mountApp("/home", { failsCreates: false, listsGhost: false, demo: { onboarded: true, agents: "codex" } });
    const mention = await waitFor(() => {
      const el = q("[data-ask-mention]");
      expect(el).not.toBeNull();
      return el!;
    });
    expect(mention.textContent).not.toMatch(/claude/i);
    expect(q("[data-ask-install]")).toBeNull();
  });

  it("with Claude Code on this computer: @claude", { timeout: 30000 }, async () => {
    await mountApp("/home", { failsCreates: false, listsGhost: false, demo: { onboarded: true } });
    await waitFor(() => expect(q("[data-ask-mention]")?.textContent?.toLowerCase()).toContain("@claude"));
  });
});
