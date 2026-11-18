// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Home turns a switched-off agent home back on in the background, once, and touches nothing else.

import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useKeepHomeWarm } from "@/components/ask-first/use-keep-home-warm";
import { rememberHome, resetHomes } from "@/lib/agent-home/home-space";

const state = vi.hoisted(() => ({
  spaces: [] as { id: string; status: string }[],
  startSpace: vi.fn(async (_id: string) => {}),
  homes: [] as string[],
}));

vi.mock("@/bridge", () => ({ useSpaces: () => ({ data: state.spaces, startSpace: state.startSpace }) }));
vi.mock("@/components/children/use-children", () => ({ useChildren: () => ({ homesOf: () => state.homes }) }));

beforeEach(() => {
  localStorage.clear();
  resetHomes();
  state.startSpace.mockClear();
  state.homes = [];
  state.spaces = [];
});

describe("keeping the home warm", () => {
  it("starts a suspended home once, and not a running one", () => {
    rememberHome("claude-code", "local:home-a");
    state.spaces = [{ id: "local:home-a", status: "suspended" }];
    const { rerender } = renderHook(() => useKeepHomeWarm("claude-code", true));
    expect(state.startSpace).toHaveBeenCalledWith("local:home-a");
    rerender();
    expect(state.startSpace).toHaveBeenCalledTimes(1);
    state.startSpace.mockClear();
    state.spaces = [{ id: "local:home-a", status: "running" }];
    rerender();
    expect(state.startSpace).not.toHaveBeenCalled();
  });

  it("finds a home by its label when this device remembers none", () => {
    state.homes = ["local:home-z"];
    state.spaces = [{ id: "local:home-z", status: "suspended" }];
    renderHook(() => useKeepHomeWarm("claude-code", true));
    expect(state.startSpace).toHaveBeenCalledWith("local:home-z");
  });

  it("does nothing when there is no home, or the feature is off", () => {
    state.spaces = [{ id: "local:other", status: "suspended" }];
    renderHook(() => useKeepHomeWarm("claude-code", true));
    rememberHome("claude-code", "local:other");
    renderHook(() => useKeepHomeWarm("claude-code", false));
    expect(state.startSpace).not.toHaveBeenCalled();
  });
});
