// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The agent chip of the ask box: the logo names the app, so a reasoning model reads "Sonnet 5.5  High"
// (the level quiet) and any other "OpenCode  Kimi K3" (the model quiet); and the menu's Reasoning row.

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { AgentAccount, AgentGroup, AgentsView } from "@/bridge";
import { t } from "@/i18n";
import { chipLine, resolveAgent } from "@/lib/ask-first-model";
import { chipWords, effortLevels, effortOf, shortModelName } from "@/lib/model-effort";

import { AgentPicker } from "../agent-picker";

afterEach(cleanup);

const account = (models: AgentAccount["models"]): AgentAccount => ({
  provider: "anthropic",
  providerLabel: "Anthropic",
  icon: "anthropic",
  env: "KEY",
  slot: null,
  line: "personal",
  via: "sign-in",
  selected: true,
  models,
  defaultModel: models[0]!.id,
});
const group = (app: string, harness: string, label: string, models: AgentAccount["models"]): AgentGroup => ({
  harness,
  app,
  label,
  state: "connected",
  accounts: [account(models)],
  signInLabel: "Sign in",
});
const CLAUDE = group("claude-code", "claude-code", "Claude Code", [
  { id: "claude-sonnet-5-5", label: "Claude Sonnet 5.5", fast: false, default: true },
  { id: "claude-haiku-5-5", label: "Claude Haiku 5.5", fast: true, default: false },
]);
const OPENCODE = group("opencode", "opencode", "OpenCode", [{ id: "kimi-k3", label: "Kimi K3", fast: false, default: true }]);
const CODEX = group("codex", "openai-codex", "OpenAI Codex", [{ id: "gpt-5.5", label: "GPT-5.5", fast: false, default: true }]);
const view = (groups: AgentGroup[]): AgentsView => ({ groups, default: null, empty: null });

function mount(g: AgentGroup, pick: { model?: string; effort?: string } = {}, onEffort = vi.fn()) {
  const v = view([g]);
  const selected = resolveAgent(v, { harness: g.harness, env: "KEY", ...pick })!;
  const r = render(<AgentPicker view={v} selected={selected} onPick={vi.fn()} onModel={vi.fn()} onEffort={onEffort} onSignIn={vi.fn()} />);
  return { ...r, selected, onEffort };
}
const chip = () => document.querySelector<HTMLElement>('[data-picker="agent"]')!;
const open = async () =>
  act(async () => {
    fireEvent.click(chip());
  });

describe("the words of the chip", () => {
  it("names a reasoning model and its level, the level quiet", () => {
    mount(CLAUDE);
    expect(chip().querySelector("[data-chip-lead]")!.textContent).toBe("Sonnet 5.5");
    expect(chip().querySelector("[data-chip-trail]")!.textContent).toBe("High");
    expect(chip().querySelector("[data-chip-trail]")!.className).toContain("text-muted-foreground");
    expect(chip().querySelector("[data-chip-lead]")!.className).not.toContain("text-muted-foreground");
  });

  it("follows the level the person picked", () => {
    mount(CLAUDE, { effort: "max" });
    expect(chip().querySelector("[data-chip-trail]")!.textContent).toBe("Max");
  });

  it("names the app and the model, the model quiet, when the model has no levels", () => {
    mount(OPENCODE);
    expect(chip().querySelector("[data-chip-lead]")!.textContent).toBe("OpenCode");
    expect(chip().querySelector("[data-chip-trail]")!.textContent).toBe("Kimi K3");
    mount(CLAUDE, { model: "claude-haiku-5-5" });
    expect(chipLine(resolveAgent(view([CLAUDE]), { harness: "claude-code", env: "KEY", model: "claude-haiku-5-5" })!, t)).toEqual({
      lead: "Claude Code",
      trail: "Haiku 5.5",
    });
  });

  it("never puts a dot or a pipe between the parts", () => {
    mount(CLAUDE);
    expect(chip().textContent).not.toMatch(/[·|]/);
  });
});

describe("the reasoning data", () => {
  it("is offered for Claude 5 and Codex models only", () => {
    expect(effortLevels("claude-code", "claude-sonnet-5-5")?.default).toBe("high");
    expect(effortLevels("claude-code", "claude-haiku-5-5")).toBeNull();
    expect(effortLevels("claude-code", "claude-haiku-4-5")).toBeNull();
    expect(effortLevels("claude-code", "claude-sonnet-5")).toBeNull();
    expect(effortLevels("openai-codex", "gpt-5.5")?.levels).not.toContain("max");
    expect(effortLevels("opencode", "kimi-k3")).toBeNull();
  });
  it("keeps a chosen level only while the model offers it", () => {
    expect(effortOf("openai-codex", "gpt-5.5", "max")).toBe("medium");
    expect(effortOf("claude-code", "claude-opus-5-5", "max")).toBe("max");
  });
  it("shortens model names", () => {
    expect(shortModelName("Claude Sonnet 5.5")).toBe("Sonnet 5.5");
    expect(chipWords("Claude Code", "GPT-5.5", "medium", t)).toEqual({ lead: "GPT-5.5", trail: "Medium" });
  });
});

describe("the Reasoning row of the menu", () => {
  it("sits right after the Model row, inline, with the level in use marked", async () => {
    mount(CLAUDE);
    await open();
    const model = document.querySelector("[data-agent-model-row]")!;
    const row = document.querySelector("[data-agent-effort-row]")!;
    expect(model.nextElementSibling).toBe(row);
    const options = [...row.querySelectorAll<HTMLElement>("[data-effort-option]")];
    expect(options.map((o) => o.textContent)).toEqual(["Low", "Med", "High", "XHigh", "Max"]);
    expect(options.filter((o) => o.getAttribute("aria-checked") === "true").map((o) => o.dataset.effortOption)).toEqual(["high"]);
    expect(row.querySelector("svg")).toBeNull();
  });

  it("picks a level in place", async () => {
    const { onEffort } = mount(CLAUDE);
    await open();
    await act(async () => {
      fireEvent.click(document.querySelector('[data-effort-option="low"]')!);
    });
    expect(onEffort).toHaveBeenCalledWith("low");
  });

  it("is absent for a model or an app without levels", async () => {
    mount(CLAUDE, { model: "claude-haiku-5-5" });
    await open();
    expect(document.querySelector("[data-agent-effort-row]")).toBeNull();
    cleanup();
    mount(OPENCODE);
    await open();
    expect(document.querySelector("[data-agent-effort-row]")).toBeNull();
  });

  it("is there for Codex's GPT models", async () => {
    mount(CODEX);
    await open();
    expect(document.querySelectorAll("[data-effort-option]").length).toBe(4);
  });
});
