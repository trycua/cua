// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { beforeEach, describe, expect, it } from "vitest";

import type { ModelGroup, ModelsView } from "@/bridge";

import { harnessFor, pickedModel, rememberModel, resetPickedModel, resolveModel } from "./ask-first-model";

const group = (provider: string, models: [string, boolean][], extra: Partial<ModelGroup> = {}): ModelGroup => ({
  provider,
  label: provider,
  icon: provider,
  state: "connected",
  harness: null,
  models: models.map(([id, fast]) => ({ id, label: id, fast, default: false })),
  ...extra,
});
const view = (groups: ModelGroup[], def: ModelsView["default"] = null): ModelsView => ({ groups, default: def, connectLabel: "Connect a provider", empty: null });

beforeEach(() => {
  localStorage.clear();
  resetPickedModel();
});

describe("which coding app runs a model", () => {
  it("is the one a sign-in belongs to, else the one that reads the provider's key best, else the general one", () => {
    expect(harnessFor({ provider: "anthropic", harness: "claude-code" })).toBe("claude-code");
    expect(harnessFor({ provider: "openai", harness: "openai-codex" })).toBe("openai-codex");
    expect(harnessFor({ provider: "anthropic", harness: null })).toBe("claude-code");
    expect(harnessFor({ provider: "openai", harness: null })).toBe("openai-codex");
    expect(harnessFor({ provider: "google", harness: null })).toBe("gemini-cli");
    expect(harnessFor({ provider: "deepseek", harness: null })).toBe("opencode");
  });
});

describe("the model a task starts with", () => {
  const v = view([group("anthropic", [["haiku", true], ["sonnet", false]]), group("deepseek", [["flash", true]])], { provider: "anthropic", model: "haiku" });

  it("is the host's default until the person picks one", () => {
    expect(resolveModel(v, null)).toMatchObject({ model: { id: "haiku" }, harness: "claude-code" });
  });

  it("is the last pick while that is still connected, and the default when it is not", () => {
    expect(resolveModel(v, { provider: "deepseek", model: "flash" })).toMatchObject({ model: { id: "flash" }, harness: "opencode" });
    expect(resolveModel(v, { provider: "openai", model: "gpt" })?.model.id).toBe("haiku");
    const reconnect = view([group("anthropic", [["haiku", true]], { state: "needs-reconnect" }), group("deepseek", [["flash", true]])], { provider: "deepseek", model: "flash" });
    expect(resolveModel(reconnect, { provider: "anthropic", model: "haiku" })?.model.id).toBe("flash");
  });

  it("is nothing when nothing is connected", () => {
    expect(resolveModel(view([]), null)).toBeNull();
    expect(resolveModel(null, null)).toBeNull();
  });

  it("keeps the pick on this computer", () => {
    expect(pickedModel()).toBeNull();
    rememberModel({ provider: "deepseek", model: "flash" });
    resetPickedModel();
    expect(pickedModel()).toEqual({ provider: "deepseek", model: "flash" });
  });
});
