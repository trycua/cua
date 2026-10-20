// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { AgentSetupRow, Space } from "@/bridge";
import { RunAgentDialog, runAgentPrompt } from "@/components/agents/run-agent-dialog";
import { SpaceReadyCard } from "@/components/space-ready-card";
import { EXPLAINED_KEY, needsExplainer, TeleportExplainer } from "@/components/teleport/explainer";

const nav = vi.fn();
let rows: AgentSetupRow[] = [];
const configureAgents = vi.fn(async () => rows);

vi.mock("@tanstack/react-router", () => ({ useNavigate: () => nav }));
vi.mock("@/bridge", async (orig) => ({
  ...(await orig<typeof import("@/bridge")>()),
  useAgents: () => ({ agentSetup: async () => rows, configureAgents }),
}));

const space = { id: "s1", name: "Research" } as Space;
const row = (over: Partial<AgentSetupRow>): AgentSetupRow => ({ agent: "claude-code", name: "Claude Code", installed: true, configured: false, ...over }) as AgentSetupRow;

beforeEach(() => {
  nav.mockReset();
  configureAgents.mockClear();
  localStorage.clear();
});
afterEach(cleanup);

describe("SpaceReadyCard", () => {
  it("has one primary action, Run an agent here", () => {
    const onRun = vi.fn();
    render(<SpaceReadyCard space={space} onRun={onRun} />);
    expect(screen.getAllByRole("button")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Run an agent here" }));
    expect(onRun).toHaveBeenCalledOnce();
  });
});

describe("RunAgentDialog", () => {
  it("shows the request to paste when an agent is connected", async () => {
    rows = [row({ configured: true })];
    render(<RunAgentDialog open space={space} onOpenChange={() => {}} />);
    expect((await screen.findByText(runAgentPrompt("Research"))).hasAttribute("data-run-agent-prompt")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Copy and see Agents" }));
    expect(nav).toHaveBeenCalledWith({ to: "/agents" });
  });

  it("sets an agent up inline when none is connected, then shows the request", async () => {
    rows = [row({})];
    configureAgents.mockImplementationOnce(async () => (rows = [row({ configured: true })]));
    render(<RunAgentDialog open space={space} onOpenChange={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "Connect Claude Code" }));
    await waitFor(() => expect(screen.getByText(runAgentPrompt("Research"))).toBeTruthy());
    expect(configureAgents).toHaveBeenCalledOnce();
  });

  it("says so when no agent is installed", async () => {
    rows = [];
    render(<RunAgentDialog open space={space} onOpenChange={() => {}} onManual={() => {}} />);
    expect(await screen.findByRole("button", { name: "Show me how" })).toBeTruthy();
  });
});

describe("Teleport explainer", () => {
  it("explains teleport and the Mac's permission prompt in plain words", () => {
    render(<TeleportExplainer />);
    const text = screen.getByText(/Your Mac will ask for permission/).closest("p")?.textContent ?? "";
    expect(text).toMatch(/Keychain/);
    expect(document.body.textContent).not.toMatch(/relay|MCP|\bhost\b|machine/i);
  });

  it("shows once on a Mac, never elsewhere", () => {
    const mac = vi.spyOn(globalThis.navigator, "platform", "get").mockReturnValue("MacIntel");
    expect(needsExplainer()).toBe(true);
    localStorage.setItem(EXPLAINED_KEY, "1");
    expect(needsExplainer()).toBe(false);
    mac.mockRestore();
  });
});
