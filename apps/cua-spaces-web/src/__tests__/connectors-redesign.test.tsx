// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, Connections: each coding app has Connect (Cua's
// tools and its sign-in, once) and a chevron to its own page; the
// Connections list has Connect, Reconnect or Disconnect. Connect on Claude Code asks first, then imports the
// sign-in Claude Code already has (`signIn: true` for Claude Code alone) and
// connects Anthropic too (claude-import.test.tsx walks every state of it).
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

/** Opens a provider's sheet from its row and disconnects it there. */
const disconnect = async (id: string) => {
  await click(await found(`[data-connector-provider=${id}]`));
  await click(await found(`[data-provider-detail=${id}] [data-connector-provider-action='sign-out']`));
};
/** Show all, so the search and the rest of the providers are there. */
const openMore = async () => {
  const toggle = await found("[data-connectors-more-toggle]");
  if (toggle.getAttribute("aria-expanded") !== "true") await click(toggle);
};

function host(calls: { op: string; args: unknown }[]) {
  return { failsCreates: false, listsGhost: false, intercept: (op: string, args: unknown) => (calls.push({ op, args }), undefined) };
}

describe.skipIf(!wasmBuilt)("Settings, Connections", () => {
  it("lists the connections, then apps with Connect and a chevron; no switches per app", async () => {
    await mountApp("/settings/connections", host([]));
    const claude = await found("[data-connector-app=claude-code]");
    expect(claude.getAttribute("role")).toBe("link");
    expect(claude.className).toContain("hover:bg-accent/50");
    expect(claude.querySelector("[data-connector-chevron]")).not.toBeNull();
    expect(claude.querySelector("[data-connector-connect]")?.textContent).toBe("Connect");
    expect(claude.textContent).toContain("Needs a sign-in to Anthropic.");
    expect(qa("[data-connectors-apps] [role=switch]").length).toBe(1); // Set up new apps automatically only
    expect(qa("[data-connector-provider]").map((r) => r.dataset.connectorProvider)).toEqual(["anthropic", "openai", "openrouter", "google", "xai", "deepseek", "kimi", "custom"]);
    expect(q("[data-connector-provider=anthropic]")!.textContent).toContain("Not connected");
    // Compact rows like the coding apps: a check and Connected, a chevron to the sheet, and nothing else on the row.
    const openai = q("[data-connector-provider=openai]")!;
    expect(openai.textContent).toContain("Connected");
    expect(openai.querySelector("[data-connector-chevron]")).not.toBeNull();
    expect(openai.textContent).not.toContain("Disconnect");
    for (const r of qa("[data-connector-provider]")) expect(r.textContent).not.toMatch(/Works with|Added /);
    expect(document.body.textContent).not.toContain("Account keys");
  });

  it("lists every agent harness without Show all, installed or not, each with Connect when not connected", async () => {
    await mountApp("/settings/connections", host([]));
    await found("[data-connector-app=claude-code]");
    const ids = qa("[data-connectors-apps] [data-connector-app]").map((r) => r.dataset.connectorApp);
    // Hermes, OpenCode, Goose and the rest are there with Show all still closed.
    for (const harness of ["claude-code", "codex", "gemini-cli", "antigravity", "opencode", "goose", "pi", "hermes", "openclaw"]) expect(ids).toContain(harness);
    expect(q("[data-connected-show-all]")!.getAttribute("aria-expanded")).toBe("false");
    // Not installed on this machine, not connected: Connect (never a bare Get it) and what it needs.
    const gemini = q("[data-connector-app=gemini-cli]")!;
    expect(gemini.dataset.state).toBe("not-connected");
    expect(gemini.querySelector("[data-connector-connect]")?.textContent).toBe("Connect");
    expect(gemini.textContent).toContain("Needs an API key for Google AI Studio.");
    expect(gemini.textContent).not.toContain("Get it");
    // The demo has an OpenAI key saved: the apps that read it are connected, and say where they run.
    expect(q("[data-connector-app=goose]")!.dataset.state).toBe("connected");
    expect(q("[data-connector-app=goose]")!.textContent).toContain("Runs in your Spaces");
    // Show all keeps the apps that cannot run as agents, with Get it.
    await click(q("[data-connected-show-all]")!);
    await found("[data-connected-missing]");
    const more = qa("[data-connected-missing]").map((r) => r.dataset.connectedMissing);
    expect(more).toContain("cline");
    for (const harness of ["goose", "opencode", "hermes", "pi", "antigravity", "gemini-cli"]) expect(more).not.toContain(harness);
  });

  it("Connect on an agent that is not installed here asks for a provider and a key, and connects what it can read", async () => {
    const calls: { op: string; args: unknown }[] = [];
    await mountApp("/settings/connections", host(calls));
    // The demo has an OpenAI key saved; sign out first so nothing reads one.
    await disconnect("openai");
    await waitFor(() => expect(q("[data-connector-app=opencode]")!.dataset.state).toBe("not-connected"));
    await click(q("[data-connector-app=opencode] [data-connector-connect]")!);
    // The sheet for its first provider, with the others one click away in a searchable choice.
    await found("[data-provider-sheet=anthropic]");
    await click(q("[data-provider-switch]")!);
    await found("[data-provider-picker]");
    expect(qa("[data-provider-choice]").map((b) => b.dataset.providerChoice)).toEqual(["anthropic", "openai", "openrouter", "google", "xai", "deepseek", "kimi", "custom"]);
    fireEvent.change(q("[data-provider-picker-search]")!, { target: { value: "openr" } });
    await waitFor(() => expect(qa("[data-provider-choice]").map((b) => b.dataset.providerChoice)).toEqual(["openrouter"]));
    await click(q("[data-provider-choice=openrouter]")!);
    await found("[data-provider-sheet=openrouter]");
    await click(q("[data-provider-way=key]")!);
    fireEvent.change(await found("[data-provider-key]"), { target: { value: "sk-or-v1-0123456789abcdef" } });
    await click(q("[data-provider-save]")!);
    // OpenRouter is a key the harness reads: it is connected, and so are the others that read it.
    await waitFor(() => expect(q("[data-connector-app=opencode]")!.dataset.state).toBe("connected"));
    expect(q("[data-connector-app=opencode]")!.textContent).toContain("Runs in your Spaces");
    expect(q("[data-connector-app=goose]")!.dataset.state).toBe("connected");
    expect(q("[data-connector-app=claude-code]")!.dataset.state).not.toBe("connected");
    // Nothing was run on this machine for it: no sign-in command, no tools written.
    expect(calls.filter((c) => c.op === "agents.connect")).toEqual([]);
  });

  it("Connect on Claude Code imports its own sign-in once you agree, and connects Anthropic", async () => {
    const calls: { op: string; args: unknown }[] = [];
    await mountApp("/settings/connections", host(calls));
    const btn = await found("[data-connector-app=claude-code] [data-connector-connect]");
    await click(btn);
    await click(await found("[data-import-confirm]"));
    await waitFor(() => expect(q("[data-connector-app=claude-code]")!.dataset.state).toBe("connected"));
    await waitFor(() => expect(q("[data-connector-provider=anthropic]")!.dataset.state).toBe("connected"));
    expect(q("[data-connector-provider=anthropic]")!.textContent).toContain("Signed in as ada@example.com (Claude Max)");
    const connects = calls.filter((c) => c.op === "agents.connect").map((c) => c.args as { agents: string[]; signIn: boolean });
    expect(connects).toEqual([{ agents: ["claude-code"], signIn: true }]);
  });

  it("a provider without a sign-in of its own asks for a key; Disconnect removes it", async () => {
    await mountApp("/settings/connections", host([]));
    // GitHub is in More providers: search for it, then Connect.
    await openMore();
    fireEvent.change(await found("[data-connectors-search]"), { target: { value: "copilot" } });
    await click(await found("[data-connector-provider=github] [data-connector-provider-action]"));
    await found("[data-provider-sheet=github]");
    expect(qa("[data-provider-way]").map((w) => w.dataset.providerWay)).toEqual(["key"]);
    await click(q("[data-provider-way=key]")!);
    fireEvent.change(await found("[data-provider-key]"), { target: { value: "github_pat_11ABCDEFG0123456789" } });
    await click(q("[data-provider-save]")!);
    await waitFor(() => expect(q("[data-connector-provider=github]")!.dataset.state).toBe("connected"));
    expect(q("[data-provider-sheet]")).toBeNull();
    await disconnect("github");
    await waitFor(() => expect(q("[data-connector-provider=github]")!.dataset.state).toBe("not-connected"));
  });

  it("the row opens the app's page: switches, its sign-in, and Remove", async () => {
    await mountApp("/settings/connections", host([]));
    await click(await found("[data-connector-app=claude-code]"));
    await found("[data-connector-detail=claude-code]");
    expect(qa("[data-connector-toggle]").map((t) => t.dataset.connectorToggle)).toEqual(["skills", "mcp", "auto"]);
    expect(q("[data-connector-detail-signin=anthropic]")!.textContent).toContain("Signs in with Anthropic");
    expect(q("[data-connector-detail-signin-action]")!.textContent).toBe("Sign in");
    expect(q("[data-connector-remove]")!.textContent).toBe("Remove Cua from Claude Code");
  });

  it("Codex signs in with its own device code: the code shows, then OpenAI is connected", async () => {
    const calls: { op: string; args: unknown }[] = [];
    await mountApp("/settings/connections", host(calls));
    // OpenAI starts connected in the demo with a key; sign out first.
    await disconnect("openai");
    await waitFor(() => expect(q("[data-connector-app=codex]")!.dataset.state).toBe("not-connected"));
    await click(q("[data-connector-app=codex] [data-connector-connect]")!);
    const code = await found("[data-device-code]");
    expect(code.textContent).toBe("QX7P-4M2KD");
    expect(q("[data-device-sign-in=codex]")?.textContent).toContain("Open sign-in page");
    await waitFor(() => expect(q("[data-connector-provider=openai]")!.dataset.state).toBe("connected"), { timeout: 8000 });
    expect(q("[data-connector-provider=openai]")!.textContent).toContain("Signed in with ChatGPT, for Codex");
    expect(q("[data-device-sign-in]")).toBeNull();
    // Never Codex's own files: no agents.connect with signIn for Codex.
    const signIns = calls.filter((c) => c.op === "agents.connect").map((c) => c.args as { agents: string[]; signIn: boolean });
    expect(signIns.every((a) => !(a.agents.includes("codex") && a.signIn))).toBe(true);
    expect(calls.some((c) => c.op === "agents.signInStart" && (c.args as { agent: string }).agent === "codex")).toBe(true);
  });

  it("a key exported in your shell can be used; only its name is read first", async () => {
    const calls: { op: string; args: unknown }[] = [];
    await mountApp("/settings/connections", host(calls));
    await click(await found("[data-connector-provider=anthropic] [data-connector-provider-action]"));
    const way = await found("[data-provider-way=shell]");
    expect(way.textContent).toContain("Use ANTHROPIC_API_KEY from your shell");
    await click(way);
    await waitFor(() => expect(q("[data-connector-provider=anthropic]")!.dataset.state).toBe("connected"));
    expect(calls.filter((c) => c.op.startsWith("agentKeys.") && c.op !== "agentKeys.list" && c.op !== "agentKeys.check").map((c) => c.op)).toEqual(["agentKeys.shell", "agentKeys.fromShell"]);
  });

  it("an app's page switches one part off and on, leaving the other", async () => {
    const calls: { op: string; args: unknown }[] = [];
    await mountApp("/settings/connections/claude-code", host(calls));
    await click(await found("[data-connector-toggle=mcp]"));
    await waitFor(() => expect(calls.some((c) => c.op === "agents.disconnect")).toBe(true));
    expect(calls.find((c) => c.op === "agents.disconnect")!.args).toEqual({ agents: ["claude-code"], signIn: false, skills: false, mcp: true });
    await waitFor(() => expect(q("[data-connector-toggle=mcp]")!.getAttribute("aria-checked") ?? q("[data-connector-toggle=mcp]")!.dataset.checked).toMatch(/false|unchecked/));
    expect(q("[data-connector-toggle=skills]")!.getAttribute("aria-checked") ?? "true").not.toBe("false");
  });
});
