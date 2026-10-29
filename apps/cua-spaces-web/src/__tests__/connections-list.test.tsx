// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, Connections: one list of providers with their state and the apps
// each works with, a searchable More providers, and a Connect sheet that
// offers only the methods that apply. Saving never waits on the provider.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { DEMO_THREAD_SPACE } from "@/bridge/adapters/demo/thread";
import { PROVIDER_REGISTRY } from "@/bridge/generated/providers";
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

type Call = { op: string; args: unknown };
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
const type = (el: Element, value: string) => fireEvent.change(el, { target: { value } });
const host = (calls: Call[], demo: object = {}) => ({ failsCreates: false, listsGhost: false, demo, intercept: (op: string, args: unknown) => (calls.push({ op, args }), undefined) });
const sets = (calls: Call[]) => calls.filter((c) => c.op === "agentKeys.set").map((c) => c.args as Record<string, unknown>);

describe.skipIf(!wasmBuilt)("the Connections list", () => {
  it("shows the eight up front with the apps each works with, and the apps open their pages", async () => {
    await mountApp("/settings/connections", host([]));
    await found("[data-connector-provider=anthropic]");
    expect(qa("[data-connectors-providers] [data-connector-provider]").map((r) => r.dataset.connectorProvider)).toEqual(
      PROVIDER_REGISTRY.filter((p) => p.tier === "top").map((p) => p.id),
    );
    const anthropic = q("[data-connector-provider=anthropic]")!;
    expect(anthropic.querySelector("[data-connector-works-with]")!.textContent).toContain("Works with");
    // Claude Code takes Anthropic and no other provider on the list.
    expect(qa("[data-connector-chip=claude-code]").map((c) => c.closest("[data-connector-provider]")!.getAttribute("data-connector-provider"))).toEqual(["anthropic"]);
    // The chips are four and a count; a chip opens its app's page.
    expect(anthropic.querySelectorAll("[data-connector-chip]").length).toBe(4);
    expect(anthropic.querySelector("[data-connector-chips-more]")!.textContent).toBe("and 3 more");
    const nav = await found("[data-connector-provider=anthropic] [data-connector-chip=claude-code]");
    await click(nav);
    await found("[data-connector-detail=claude-code]");
  });

  it("keeps the rest in More providers: closed, then all of them, searched by name or alias", async () => {
    await mountApp("/settings/connections", host([]));
    await found("[data-connectors-more]");
    expect(qa("[data-connectors-more] [data-connector-provider]")).toHaveLength(0);
    const toggle = q("[data-connectors-more-toggle]")!;
    expect(toggle.textContent).toMatch(/^Show all \d+$/);
    await click(toggle);
    const all = await waitFor(() => {
      const rows = qa("[data-connectors-more] [data-connector-provider]").map((r) => r.dataset.connectorProvider);
      expect(rows.length).toBeGreaterThan(15);
      return rows;
    });
    for (const id of ["huggingface", "zai", "fireworks", "nvidia", "ollama-cloud"]) expect(all).toContain(id);
    // Unreviewed logins and providers Cua hasn't run yet never show.
    for (const id of ["nous", "qwen-oauth", "minimax-oauth", "xai-oauth", "vertex", "bedrock", "azure-foundry", "meta-ai"]) expect(all).not.toContain(id);
    // Search by alias: HF is Hugging Face, glm is Z.AI.
    type(q("[data-connectors-search]")!, "hf");
    await waitFor(() => expect(qa("[data-connectors-more] [data-connector-provider]").map((r) => r.dataset.connectorProvider)).toEqual(["huggingface"]));
    type(q("[data-connectors-search]")!, "glm");
    await waitFor(() => expect(qa("[data-connectors-more] [data-connector-provider]").map((r) => r.dataset.connectorProvider)).toEqual(["zai"]));
    type(q("[data-connectors-search]")!, "no such thing");
    await found("[data-connectors-more-empty]");
    expect(q("[data-connectors-more-empty]")!.textContent).toBe("No provider matches that.");
  });

  it("an OpenRouter key connects it with one paste, saves without waiting for the provider, and Hermes works with it", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connections", host(calls));
    await click(await found("[data-connector-provider=openrouter] [data-connector-provider-action]"));
    await found("[data-provider-sheet=openrouter]");
    // Only the way that applies, with who uses it.
    expect(qa("[data-provider-way]").map((w) => w.dataset.providerWay)).toEqual(["key"]);
    expect(q("[data-provider-sheet=openrouter]")!.textContent).toContain("Works with");
    await click(q("[data-provider-way=key]")!);
    expect((q("[data-provider-save]") as HTMLButtonElement).disabled).toBe(true);
    type(await found("[data-provider-key]"), "sk-or-v1-0123456789abcdef");
    const before = calls.length;
    await click(q("[data-provider-save]")!);
    await waitFor(() => expect(q("[data-connector-provider=openrouter]")!.dataset.state).toBe("connected"));
    expect(sets(calls)).toEqual([{ provider: "openrouter", value: "sk-or-v1-0123456789abcdef", env: null }]);
    // Nothing asked the provider before the key was saved: no check between Save and the write.
    const between = calls.slice(before).map((c) => c.op);
    expect(between.indexOf("agentKeys.set")).toBe(0);
    const row = q("[data-connector-provider=openrouter]")!;
    expect(row.textContent).toContain("Connected");
    expect(row.querySelector("[data-connector-credential]")!.textContent).toContain("•••• cdef");
    expect(row.textContent).toContain("Disconnect");
    // Hermes (and the other apps that read an OpenRouter key) are connected by it.
    expect(q("[data-connector-app=goose]")!.dataset.state).toBe("connected");
    expect(q("[data-connector-provider=openrouter] [data-connector-chip=hermes]")).not.toBeNull();
  });

  it("a custom endpoint asks for its URL, model and API style, needs no key, and says what a local server needs", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connections", host(calls));
    await click(await found("[data-connector-provider=custom] [data-connector-provider-action]"));
    await found("[data-provider-sheet=custom]");
    await click(q("[data-provider-way=endpoint]")!);
    expect(qa("[data-provider-field]").map((f) => f.dataset.providerField)).toEqual(["base_url", "model", "wire"]);
    expect(q("[data-provider-note]")!.textContent).toContain("only reachable from a Space that is allowed to reach that port");
    expect(q("[data-provider-sheet=custom]")!.textContent).toContain("API key (optional)");
    // Not ready until the URL and model are there; the key stays empty.
    const save = () => q("[data-provider-save]") as HTMLButtonElement;
    expect(save().disabled).toBe(true);
    type(q("[data-provider-field=base_url] input")!, "http://127.0.0.1:1234/v1");
    expect(save().disabled).toBe(true);
    type(q("[data-provider-field=model] input")!, "qwen3-coder");
    await waitFor(() => expect(save().disabled).toBe(false));
    await click(save());
    await waitFor(() => expect(q("[data-connector-provider=custom]")!.dataset.state).toBe("connected"));
    expect(sets(calls)).toEqual([{ provider: "custom", value: "", env: null, fields: { base_url: "http://127.0.0.1:1234/v1", model: "qwen3-coder", wire: "openai-chat" } }]);
    expect(q("[data-connector-provider=custom] [data-connector-credential]")!.textContent).toBe("http://127.0.0.1:1234/v1");
    // Hermes and the apps that can be pointed at an endpoint are connected by it.
    for (const app of ["opencode", "goose"]) expect(q(`[data-connector-app=${app}]`)!.dataset.state).toBe("connected");
    expect(q("[data-connector-provider=custom] [data-connector-chip=hermes]")).not.toBeNull();
    expect(q("[data-connector-app=claude-code]")!.dataset.state).not.toBe("connected");
  });

  it("Z.AI asks which plan, because the plans bill separately", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connections", host(calls));
    type(await found("[data-connectors-search]"), "z.ai");
    await click(await found("[data-connector-provider=zai] [data-connector-provider-action]"));
    await found("[data-provider-sheet=zai]");
    await click(q("[data-provider-way=key]")!);
    expect(q("[data-provider-field=region]")!.textContent).toContain("Plan");
    expect(q("[data-provider-field=region]")!.textContent).toContain("Global");
    type(await found("[data-provider-key]"), "zai-key-0123456789");
    await click(q("[data-provider-save]")!);
    await waitFor(() => expect(q("[data-connector-provider=zai]")!.dataset.state).toBe("connected"));
    expect(sets(calls)).toEqual([{ provider: "zai", value: "zai-key-0123456789", env: null, region: "global" }]);
    expect(q("[data-connector-provider=zai] [data-connector-credential]")!.textContent).toContain("Global");
  });

  it("Other providers in a sheet opens the choice: the eight, then More providers, searchable", async () => {
    await mountApp("/settings/connections", host([]));
    await click(await found("[data-connector-provider=anthropic] [data-connector-provider-action]"));
    await found("[data-provider-sheet=anthropic]");
    await click(q("[data-provider-switch]")!);
    await found("[data-provider-picker]");
    expect(qa("[data-provider-choice]")).toHaveLength(8);
    await click(q("[data-provider-picker-more]")!);
    await waitFor(() => expect(qa("[data-provider-choice]").length).toBeGreaterThan(15));
    type(q("[data-provider-picker-search]")!, "nvid");
    await waitFor(() => expect(qa("[data-provider-choice]").map((c) => c.dataset.providerChoice)).toEqual(["nvidia"]));
    await click(q("[data-provider-choice=nvidia]")!);
    await found("[data-provider-sheet=nvidia]");
  });

  it("Disconnect removes it, Needs reconnect reads on the row, and a connected provider from More stays up front", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connections", host(calls, { rejectedKeys: ["OPENAI_API_KEY"] }));
    await waitFor(() => expect(q("[data-connector-provider=openai]")?.textContent).toContain("Needs reconnect"));
    expect(q("[data-connector-provider=openai] [data-connector-provider-action]")!.textContent).toBe("Reconnect");
    // Save a Fireworks key: it joins the list up front.
    type(await found("[data-connectors-search]"), "fireworks");
    await click(await found("[data-connectors-more] [data-connector-provider=fireworks] [data-connector-provider-action]"));
    await click(await found("[data-provider-way=key]"));
    type(await found("[data-provider-key]"), "fw_0123456789abcdef");
    await click(q("[data-provider-save]")!);
    await waitFor(() => expect(q("[data-connectors-providers] [data-connector-provider=fireworks]")?.dataset.state).toBe("connected"));
    await click(q("[data-connectors-providers] [data-connector-provider=fireworks] [data-connector-provider-action]")!);
    await waitFor(() => expect(q("[data-connector-provider=fireworks]")?.dataset.state ?? "gone").not.toBe("connected"));
    expect(calls.some((c) => c.op === "agentKeys.remove" && (c.args as { env: string }).env === "FIREWORKS_API_KEY")).toBe(true);
  });

  it("says machines, never devices, and has no dot separators", async () => {
    await mountApp("/settings/connections", host([]));
    await found("[data-connector-provider=anthropic]");
    await click(q("[data-connectors-more-toggle]")!);
    await click(await found("[data-connector-provider=anthropic] [data-connector-provider-action]"));
    await found("[data-provider-sheet=anthropic]");
    const text = document.body.textContent!;
    expect(text).not.toContain("·");
    expect(text.toLowerCase()).not.toContain("device");
  });
});

describe.skipIf(!wasmBuilt)("the Connect card in a Space thread", () => {
  const path = `/spaces/${encodeURIComponent(DEMO_THREAD_SPACE)}`;
  const reply = (who: string) => qa("[data-entry]").find((e) => e.querySelector("[data-failure-connect]") && e.textContent?.includes(who));

  it("Hermes isn't connected: Connect opens the same sheet, on a provider Hermes can use, with the others one click away", async () => {
    const calls: Call[] = [];
    await mountApp(path, host(calls, { threadAuthFailed: true }));
    const card = await waitFor(
      () => {
        const r = reply("Hermes needs a connected provider");
        expect(r).toBeTruthy();
        return r as HTMLElement;
      },
      { timeout: 5000 },
    );
    expect(card.textContent).toContain("Your Claude subscription sign-in only works in Claude Code");
    await click(card.querySelector("[data-failure-connect]")!);
    await found("[data-provider-sheet]");
    // An API key way, not the Claude sign-in: that is Claude Code's alone.
    expect(qa("[data-provider-way]").map((w) => w.dataset.providerWay)).not.toContain("app-token");
    await click(q("[data-provider-switch]")!);
    await found("[data-provider-picker]");
    expect(q("[data-provider-picker]")!.textContent).toContain("Providers Hermes can use.");
    await click(await found("[data-provider-choice=openrouter]"));
    await found("[data-provider-sheet=openrouter]");
    // Nothing was written or run for the sign-in itself.
    expect(calls.filter((c) => c.op === "agents.connect")).toEqual([]);
  });
  it("Hermes' picker and sheet tell the truth when only Claude and ChatGPT sign-ins are saved", async () => {
    await mountApp(path, host([], { threadAuthFailed: true, keys: "none", claudeLogin: "signed-in", chatgptLogin: true }));
    const card = await waitFor(
      () => {
        const r = reply("Hermes needs a connected provider");
        expect(r).toBeTruthy();
        return r as HTMLElement;
      },
      { timeout: 5000 },
    );
    await click(card.querySelector("[data-failure-connect]")!);
    const sheet = await found("[data-provider-sheet=anthropic]");
    // Only a key, and one plain sentence on why.
    expect(qa("[data-provider-way]").map((w) => w.dataset.providerWay)).toEqual(["key"]);
    expect(q("[data-provider-only-key]")!.textContent).toBe("Hermes uses an API key. Your Claude sign-in is used by Claude Code.");
    // The header is a header: a title and a small works-with line, no tile like the option rows have.
    const header = sheet.querySelector("[data-provider-sheet-header]")!;
    expect(header.querySelector("[data-harness-mark]")).toBeNull();
    expect(header.querySelector("[data-provider-works-with]")!.textContent).toMatch(/^Works with /);
    expect(sheet.textContent).not.toContain("\u00b7");
    await click(q("[data-provider-switch]")!);
    await found("[data-provider-picker]");
    for (const id of ["anthropic", "openai"]) {
      const row = await found(`[data-provider-choice=${id}]`);
      expect(row.textContent).toContain("Hermes needs an API key");
      expect(row.textContent).not.toContain("Connected");
    }
  });
});
