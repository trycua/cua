// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// An app's page says which credential it uses. Claude Code and Codex read one fixed sign-in; Hermes, OpenCode,
// Goose, Pi and OpenClaw take any provider, so their page has a Uses row with a menu of the connected providers
// and keys they can really read, and Connect another provider at the bottom. Choosing signs nothing out.

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
/** Only the Claude and ChatGPT sign-ins are saved: no API key at all. */
const SIGN_INS_ONLY = { keys: "none", claudeLogin: "signed-in", chatgptLogin: true };
const SWITCHABLE = ["hermes", "opencode", "goose", "pi", "openclaw"];

/** Opens the Uses menu (Base UI opens on a pointer press). */
const openMenu = async () => {
  const trigger = await found("[data-uses-menu]");
  await act(async () => {
    fireEvent.pointerDown(trigger, { button: 0, pointerType: "mouse" });
    fireEvent.mouseDown(trigger, { button: 0 });
    fireEvent.pointerUp(trigger, { button: 0, pointerType: "mouse" });
    fireEvent.click(trigger);
  });
  return found("[data-uses-popup]");
};

describe.skipIf(!wasmBuilt)("an app's Uses row", () => {
  it.each(SWITCHABLE)("%s with only a Claude and a ChatGPT sign-in saved is not connected, and says so", async (app) => {
    await mountApp(`/settings/connections/${app}`, host([], SIGN_INS_ONLY));
    const row = await found(`[data-connector-uses-app=${app}]`);
    expect(row.dataset.connectorUses).toBe("not-connected");
    expect(row.textContent).toContain("No provider connected");
    expect(row.textContent).toContain(`${q(`[data-connector-detail=${app}]`)!.querySelector("h2")!.textContent} needs an API key`);
    expect(row.textContent).not.toMatch(/\bUses\b/);
    // Never the Claude or ChatGPT sign-in, never Sign out on this page.
    expect(q(`[data-connector-detail=${app}]`)!.textContent).not.toMatch(/ada@example\.com|Claude Max|ChatGPT|Signed in as/);
    expect(q("[data-connector-detail-signin-action]")).toBeNull();
    expect(q("[data-uses-menu]")).toBeNull();
    expect(q("[data-connector-detail-status]")!.textContent).not.toMatch(/Signed in|Connected/);
    expect(q("[data-uses-connect]")!.textContent).toBe("Connect");
  });

  it("Claude Code keeps its fixed sign-in row, with Sign out, and no switcher", async () => {
    await mountApp("/settings/connections/claude-code", host([], SIGN_INS_ONLY));
    const row = await found("[data-connector-detail-signin]");
    expect(row.textContent).toContain("ada@example.com");
    expect(q("[data-connector-detail-signin-action]")!.textContent).toBe("Sign out");
    expect(q("[data-connector-uses-app]")).toBeNull();
    expect(q("[data-uses-menu]")).toBeNull();
  });

  it("lists the connected providers the app can read, points it at one without signing anything out, and says when it takes effect", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connections/hermes", host(calls, { keys: "several", claudeLogin: "signed-in", chatgptLogin: true }));
    const row = await found("[data-connector-uses=connected]");
    // The first connected one that works is in use until the person chooses.
    expect(row.querySelector("[data-uses-provider]")!.textContent).toBe("Anthropic");
    expect(row.querySelector("[data-uses-line]")!.textContent).toContain("API key ending a91c");
    expect(document.body.textContent).toContain("Takes effect the next time Hermes runs.");
    const popup = await openMenu();
    const options = qa("[data-uses-option]").map((o) => o.dataset.usesOption);
    expect(options).toEqual(expect.arrayContaining(["anthropic", "openrouter", "deepseek"]));
    // The Claude and ChatGPT sign-ins never appear for an app that cannot read them.
    expect(popup.textContent).not.toMatch(/ada@example\.com|ChatGPT/);
    expect(qa("[data-uses-option]").filter((o) => o.dataset.usesOption === "anthropic")).toHaveLength(1);
    expect(popup.textContent).toContain("Connect another provider");
    await click(q("[data-uses-option=openrouter]")!);
    await waitFor(() => expect(calls.filter((c) => c.op === "agentKeys.use").map((c) => c.args)).toEqual([{ app: "hermes", env: "OPENROUTER_API_KEY" }]));
    await waitFor(() => expect(q("[data-connector-uses-app=hermes] [data-uses-provider]")!.textContent).toBe("OpenRouter"));
    // Nothing was signed out or removed by choosing.
    expect(calls.filter((c) => c.op === "agentKeys.remove")).toEqual([]);
    expect(q("[data-connector-uses-app=hermes] [data-uses-line]")!.textContent).toContain("API key ending cdef");
  });

  it("Connect another provider opens the provider choice for this app", async () => {
    await mountApp("/settings/connections/hermes", host([], { keys: "several" }));
    await openMenu();
    await click(q("[data-uses-connect]")!);
    await found("[data-provider-picker], [data-provider-sheet]");
  });

  it("a removed key leaves the app not connected with a Connect state, never moved to another key", async () => {
    const calls: Call[] = [];
    const { router } = await mountApp("/settings/connections/hermes", host(calls, { keys: "several" }));
    await openMenu();
    await click(await found("[data-uses-option=deepseek]"));
    await waitFor(() => expect(q("[data-connector-uses-app=hermes] [data-uses-provider]")!.textContent).toBe("DeepSeek"));
    // Disconnect DeepSeek on the list.
    await act(async () => {
      await router.navigate({ to: "/settings/connections" });
    });
    await click(await found("[data-connector-provider=deepseek]"));
    await click(await found("[data-provider-detail=deepseek] [data-connector-provider-action='sign-out']"));
    await waitFor(() => expect(q("[data-connector-provider=deepseek]")?.dataset.state ?? "gone").not.toBe("connected"));
    await act(async () => {
      await router.navigate({ to: "/settings/connections/$app", params: { app: "hermes" } });
    });
    const row = await found("[data-connector-uses=gone]");
    expect(row.textContent).toContain("isn't saved any more");
    expect(row.textContent).toContain("Key not found");
    expect(row.textContent).not.toMatch(/\bUses\b/);
    expect(row.querySelector("[data-uses-provider]")).toBeNull();
    expect(q("[data-connector-detail-status]")!.textContent).not.toMatch(/Connected/);
    // Anthropic and the rest are still saved and still offered, but none is selected for it.
    await openMenu();
    expect(qa("[data-uses-option]").length).toBeGreaterThan(1);
    expect(qa("[data-uses-option][data-selected]")).toHaveLength(0);
  });

  it("another key for a provider is named, kept beside the first, and offered to the apps that take any provider", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connections", host(calls, { keys: "several" }));
    await click(await found("[data-connector-provider=anthropic]"));
    await click(await found("[data-connector-add-key]"));
    await found("[data-connector-add-key-sheet=anthropic]");
    type(q("[data-add-key-name]")!, "Work key");
    type(q("[data-add-key-value]")!, "sk-ant-api03-work-0123456789");
    await click(q("[data-add-key-save]")!);
    await waitFor(() => expect(q("[data-connector-add-key-sheet]")).toBeNull());
    expect(calls.filter((c) => c.op === "agentKeys.set").map((c) => c.args)).toEqual([{ provider: "anthropic", value: "sk-ant-api03-work-0123456789", env: null, add: true, label: "Work key" }]);
    // The provider's row is still the first key: nothing was replaced.
    expect(q("[data-connector-provider=anthropic] [data-connector-credential]")!.textContent).toContain("a91c");
    // Its sheet now lists both keys, each removable.
    await click(await found("[data-connector-provider=anthropic]"));
    await waitFor(() => expect(qa("[data-connector-key]")).toHaveLength(2));
    expect(q("[data-connector-keys]")!.textContent).toContain("Work key");
  });
});
