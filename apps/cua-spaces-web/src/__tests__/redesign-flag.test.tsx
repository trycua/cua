// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `spaces_redesign` (Settings → Advanced → Labs) switches whole pages: on,
// the default, the redesign; off, each gated route draws the classic page
// (src/legacy/). Both states render, so neither can quietly break.

import { cleanup, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { DEMO_THREAD_SPACE } from "@/bridge/adapters/demo/thread";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
});

const HOST = { failsCreates: false, listsGhost: false };
const q = (sel: string) => document.querySelector(sel);
const flag = (on: boolean) => vi.stubEnv("VITE_CUA_FLAG_SPACES_REDESIGN", on ? "1" : "0");

describe.skipIf(!wasmBuilt)("the redesign flag", () => {
  it("is on by default, and an override or the build's env turns it off", async () => {
    const { flagOn, setFlagOverride } = await import("@/bridge/flags");
    setFlagOverride("spaces_redesign", null);
    vi.stubEnv("VITE_CUA_FLAG_SPACES_REDESIGN", "");
    expect(flagOn("spaces_redesign")).toBe(true);
    // The build's env.
    vi.stubEnv("VITE_CUA_FLAG_SPACES_REDESIGN", "0");
    expect(flagOn("spaces_redesign")).toBe(false);
    // This device's explicit choice wins over the env, both ways.
    vi.stubEnv("VITE_CUA_FLAG_SPACES_REDESIGN", "");
    setFlagOverride("spaces_redesign", false);
    expect(flagOn("spaces_redesign")).toBe(false);
    vi.stubEnv("VITE_CUA_FLAG_SPACES_REDESIGN", "0");
    setFlagOverride("spaces_redesign", true);
    expect(flagOn("spaces_redesign")).toBe(true);
    setFlagOverride("spaces_redesign", null);
    // The Ask first Home stays off by default.
    vi.stubEnv("VITE_CUA_FLAG_SPACES_ASK_FIRST_HOME", "");
    expect(flagOn("spaces_ask_first_home")).toBe(false);
  });

  it("Spaces page: the toolbar and the new sidebar on; the page and the getting-started checklist as they were off", async () => {
    flag(true);
    await mountApp("/spaces", HOST);
    await waitFor(() => expect(q("[data-spaces-toolbar]")).not.toBeNull());
    expect(q("[data-account-line]")).not.toBeNull();
    cleanup();

    flag(false);
    await mountApp("/spaces", HOST);
    await waitFor(() => expect(q("[data-space-id]")).not.toBeNull());
    expect(q("[data-spaces-toolbar]")).toBeNull();
    expect(q("[data-account-line]")).toBeNull();
  });

  it("a Space: the thread and Stream rail on; the page as it was off", async () => {
    const path = `/spaces/${encodeURIComponent(DEMO_THREAD_SPACE)}`;
    flag(true);
    await mountApp(path, HOST);
    await waitFor(() => expect(q("[data-composer]")).not.toBeNull());
    cleanup();

    flag(false);
    await mountApp(path, HOST);
    await waitFor(() => expect(document.body.textContent).toContain("ubuntu-build"));
    expect(q("[data-composer]")).toBeNull();
  });

  it("Agents: the chat on; the page as it was off", async () => {
    flag(true);
    await mountApp("/agents", HOST);
    await waitFor(() => expect(q("[data-agents-chat]")).not.toBeNull());
    cleanup();

    flag(false);
    await mountApp("/agents", HOST);
    await waitFor(() => expect(q("[data-agents-section], [data-agents-empty]")).not.toBeNull());
    expect(q("[data-agents-chat]")).toBeNull();
  });

  it("Computers keeps the clouds (Connect a cloud is on for everyone) in both states", async () => {
    for (const on of [true, false]) {
      flag(on);
      await mountApp("/machines", HOST);
      await waitFor(() => expect(q("[data-connect-a-cloud]")).not.toBeNull());
      cleanup();
    }
  });
});
