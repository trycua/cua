// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, Connectors with the redesign: each coding app has Connect (Cua's
// tools and its sign-in, once) and a chevron to its own page; Providers has
// Connect or Sign out. Connect on Claude Code runs only its own token
// command (`signIn: true` for Claude Code alone) and connects Anthropic too.
// Off, the page as it was: switches and Account keys.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetConnectState } from "@/hooks/use-connectors";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetConnectedAppsState();
  resetConnectState();
});
afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
});

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
const found = (s: string) =>
  waitFor(() => {
    const el = q(s);
    expect(el).not.toBeNull();
    return el!;
  });
const click = (el: Element) =>
  act(async () => {
    fireEvent.click(el);
  });

function host(calls: { op: string; args: unknown }[]) {
  return { failsCreates: false, listsGhost: false, intercept: (op: string, args: unknown) => (calls.push({ op, args }), undefined) };
}

describe.skipIf(!wasmBuilt)("Settings, Connectors (redesign)", () => {
  it("lists apps with Connect and a chevron, then Providers; no switches per app", async () => {
    await mountApp("/settings/connectors", host([]));
    const claude = await found("[data-connector-app=claude-code]");
    expect(claude.getAttribute("role")).toBe("link");
    expect(claude.className).toContain("hover:bg-accent/50");
    expect(claude.querySelector("[data-connector-chevron]")).not.toBeNull();
    expect(claude.querySelector("[data-connector-connect]")?.textContent).toBe("Connect");
    expect(claude.textContent).toContain("Needs a sign-in to Anthropic.");
    expect(qa("[data-connectors-apps] [role=switch]").length).toBe(1); // Set up new apps automatically only
    expect(qa("[data-connector-provider]").map((r) => r.dataset.connectorProvider)).toEqual(["anthropic", "openai", "github"]);
    expect(q("[data-connector-provider=anthropic]")!.textContent).toContain("Not connected");
    expect(q("[data-connector-provider=openai]")!.textContent).toContain("Sign out");
    expect(document.body.textContent).not.toContain("Account keys");
  });

  it("Connect on Claude Code runs its token command alone, and connects Anthropic", async () => {
    const calls: { op: string; args: unknown }[] = [];
    await mountApp("/settings/connectors", host(calls));
    const btn = await found("[data-connector-app=claude-code] [data-connector-connect]");
    await click(btn);
    await waitFor(() => expect(q("[data-connector-app=claude-code]")!.dataset.state).toBe("connected"));
    await waitFor(() => expect(q("[data-connector-provider=anthropic]")!.dataset.state).toBe("connected"));
    expect(q("[data-connector-provider=anthropic]")!.textContent).toContain("Claude sign-in, for Claude Code");
    const connects = calls.filter((c) => c.op === "agents.connect").map((c) => c.args as { agents: string[]; signIn: boolean });
    expect(connects).toEqual([{ agents: ["claude-code"], signIn: true }]);
  });

  it("a provider without a sign-in of its own asks for a key; Sign out removes it", async () => {
    await mountApp("/settings/connectors", host([]));
    await click(await found("[data-connector-provider=github] [data-connector-provider-action]"));
    await found("[data-provider-sheet=github]");
    expect(qa("[data-provider-way]").map((w) => w.dataset.providerWay)).toEqual(["key"]);
    await click(q("[data-provider-way=key]")!);
    fireEvent.change(await found("[data-provider-key]"), { target: { value: "github_pat_11ABCDEFG0123456789" } });
    await click(q("[data-provider-save]")!);
    await waitFor(() => expect(q("[data-connector-provider=github]")!.dataset.state).toBe("connected"));
    expect(q("[data-provider-sheet]")).toBeNull();
    await click(q("[data-connector-provider=github] [data-connector-provider-action]")!);
    await waitFor(() => expect(q("[data-connector-provider=github]")!.dataset.state).toBe("not-connected"));
  });

  it("the row opens the app's page: switches, its sign-in, and Remove", async () => {
    await mountApp("/settings/connectors", host([]));
    await click(await found("[data-connector-app=claude-code]"));
    await found("[data-connector-detail=claude-code]");
    expect(qa("[data-connector-toggle]").map((t) => t.dataset.connectorToggle)).toEqual(["skills", "mcp", "auto"]);
    expect(q("[data-connector-detail-signin=anthropic]")!.textContent).toContain("Signs in with Anthropic");
    expect(q("[data-connector-detail-signin-action]")!.textContent).toBe("Sign in");
    expect(q("[data-connector-remove]")!.textContent).toBe("Remove Cua from Claude Code");
  });

  it("Codex signs in with its own device code: the code shows, then OpenAI is connected", async () => {
    const calls: { op: string; args: unknown }[] = [];
    await mountApp("/settings/connectors", host(calls));
    // OpenAI starts connected in the demo with a key; sign out first.
    await click(await found("[data-connector-provider=openai] [data-connector-provider-action='sign-out']"));
    await waitFor(() => expect(q("[data-connector-app=codex]")!.dataset.state).toBe("not-connected"));
    await click(q("[data-connector-app=codex] [data-connector-connect]")!);
    const code = await found("[data-device-code]");
    expect(code.textContent).toBe("QX7P-4M2KD");
    expect(q("[data-device-sign-in=codex]")?.textContent).toContain("Open sign-in page");
    await waitFor(() => expect(q("[data-connector-provider=openai]")!.dataset.state).toBe("connected"), { timeout: 8000 });
    expect(q("[data-connector-provider=openai]")!.textContent).toContain("ChatGPT sign-in, for Codex");
    expect(q("[data-device-sign-in]")).toBeNull();
    // Never Codex's own files: no agents.connect with signIn for Codex.
    const signIns = calls.filter((c) => c.op === "agents.connect").map((c) => c.args as { agents: string[]; signIn: boolean });
    expect(signIns.every((a) => !(a.agents.includes("codex") && a.signIn))).toBe(true);
    expect(calls.some((c) => c.op === "agents.signInStart" && (c.args as { agent: string }).agent === "codex")).toBe(true);
  });

  it("a key exported in your shell can be used; only its name is read first", async () => {
    const calls: { op: string; args: unknown }[] = [];
    await mountApp("/settings/connectors", host(calls));
    await click(await found("[data-connector-provider=anthropic] [data-connector-provider-action]"));
    const way = await found("[data-provider-way=shell]");
    expect(way.textContent).toContain("Use ANTHROPIC_API_KEY from your shell");
    await click(way);
    await waitFor(() => expect(q("[data-connector-provider=anthropic]")!.dataset.state).toBe("connected"));
    expect(calls.filter((c) => c.op.startsWith("agentKeys.") && c.op !== "agentKeys.list").map((c) => c.op)).toEqual(["agentKeys.shell", "agentKeys.fromShell"]);
  });

  it("an app's page switches one part off and on, leaving the other", async () => {
    const calls: { op: string; args: unknown }[] = [];
    await mountApp("/settings/connectors/claude-code", host(calls));
    await click(await found("[data-connector-toggle=mcp]"));
    await waitFor(() => expect(calls.some((c) => c.op === "agents.disconnect")).toBe(true));
    expect(calls.find((c) => c.op === "agents.disconnect")!.args).toEqual({ agents: ["claude-code"], signIn: false, skills: false, mcp: true });
    await waitFor(() => expect(q("[data-connector-toggle=mcp]")!.getAttribute("aria-checked") ?? q("[data-connector-toggle=mcp]")!.dataset.checked).toMatch(/false|unchecked/));
    expect(q("[data-connector-toggle=skills]")!.getAttribute("aria-checked") ?? "true").not.toBe("false");
  });

  it("redesign off: the page as it was, with switches and Account keys", async () => {
    vi.stubEnv("VITE_CUA_FLAG_SPACES_REDESIGN", "0");
    await mountApp("/settings/connectors", host([]));
    await found('[data-connected-app="claude-code"]');
    expect(q("[data-connectors-redesign]")).toBeNull();
    await found("[data-agent-keys]");
  });
});
