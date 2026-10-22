// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ReactNode } from "react";

import type { AgentSetupRow, Space } from "@/bridge";
import { BridgeContext, type BridgeContextValue } from "@/bridge/BridgeProvider";
import { RunAgentDialog, runAgentPrompt } from "@/components/agents/run-agent-dialog";
import { SpaceReadyCard, useAgentOffer } from "@/components/space-ready-card";
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
  it("lists the agents it found and has one action", async () => {
    rows = [row({}), row({ agent: "codex", name: "Codex", configured: true }), row({ agent: "hermes", name: "Hermes", installed: false })];
    const onRun = vi.fn();
    render(<SpaceReadyCard space={space} onRun={onRun} />);
    expect(await screen.findByText(/We found Claude Code and Codex on this computer/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Start with Codex" }));
    expect(onRun).toHaveBeenCalledOnce();
    expect(document.body.textContent).not.toMatch(/relay|MCP|\bhost\b/i);
  });

  it("offers to connect an agent that is installed but not connected", async () => {
    rows = [row({})];
    render(<SpaceReadyCard space={space} onRun={() => {}} />);
    expect(await screen.findByRole("button", { name: "Connect Claude Code" })).toBeTruthy();
  });

  it("says how when no agent is found, and can be closed", async () => {
    rows = [];
    const onDismiss = vi.fn();
    render(<SpaceReadyCard space={space} onRun={() => {}} onDismiss={onDismiss} />);
    expect(await screen.findByRole("button", { name: "Show me how" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(onDismiss).toHaveBeenCalledOnce();
  });
});

describe("useAgentOffer", () => {
  const track = vi.fn();
  const wrapper = ({ children }: { children?: ReactNode }) => {
    const store = { telemetry: { track } };
    const value = { mode: "electron", core: {}, adapter: null, store, ready: Promise.resolve(store) } as unknown as BridgeContextValue;
    return <BridgeContext.Provider value={value}>{children}</BridgeContext.Provider>;
  };
  const events = () => track.mock.calls.map((c) => (c[0] as { feature: string }[])[0]!.feature);
  beforeEach(() => track.mockClear());

  it("records shown once, then clicked and completed, and never shows again", async () => {
    const { result, rerender } = renderHook(() => useAgentOffer(true), { wrapper });
    await waitFor(() => expect(events()).toEqual(["agent_offer_shown"]));
    act(() => result.current.clicked());
    act(() => result.current.completed());
    await waitFor(() => expect(events()).toEqual(["agent_offer_shown", "agent_offer_clicked", "agent_offer_completed"]));
    expect(result.current.visible).toBe(false);
    rerender();
    const again = renderHook(() => useAgentOffer(true), { wrapper });
    expect(again.result.current.visible).toBe(false);
    expect(events()).toHaveLength(3);
  });

  it("stays closed after Not now, and counts shown once per install", async () => {
    const first = renderHook(() => useAgentOffer(true), { wrapper });
    await waitFor(() => expect(events()).toEqual(["agent_offer_shown"]));
    act(() => first.result.current.dismiss());
    expect(first.result.current.visible).toBe(false);
    expect(renderHook(() => useAgentOffer(true), { wrapper }).result.current.visible).toBe(false);
    expect(events()).toEqual(["agent_offer_shown"]);
  });

  it("is hidden until a Space is ready", () => {
    expect(renderHook(() => useAgentOffer(false), { wrapper }).result.current.visible).toBe(false);
    expect(track).not.toHaveBeenCalled();
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
