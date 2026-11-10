// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Which coding apps Home's agent menu shows: the "Manage agents and sign-ins" button at the foot of the menu
// leads to Settings, Connections, where each app has a switch. The rule (defaults, the last app
// that cannot be hidden, a connect that shows an app again) is the app core's `agent_menu`.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetConnectState } from "@/hooks/use-connectors";
import { resetAskTasks } from "@/lib/ask-first";
import { resetPickedAgent } from "@/lib/ask-first-model";
import { resetRepos } from "@/lib/ask-first-repo";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
  resetPickedAgent();
  resetRepos();
  resetConnectState();
  resetConnectedAppsState();
});
afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
});

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
const HOST = { failsCreates: false, listsGhost: false, demo: { accounts: true } };
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
const openMenu = async () => {
  await click(await found('[data-picker="agent"]'));
  await found("[data-agent-menu]");
};
const groups = () => qa("[data-agent-group]").map((g) => g.dataset.agentGroup);
const sw = (app: string) => found(`[data-agent-menu-switch="${app}"]`);

describe.skipIf(!wasmBuilt)("the agent menu's apps", () => {
  it("ends with a Manage agents and sign-ins button that opens Settings, Connections", async () => {
    const { router } = await mountApp("/home/ask", HOST);
    await screen.findByLabelText("What should it do?");
    await openMenu();
    const button = await found("[data-agent-configure]");
    expect(button.textContent).toBe("Manage agents and sign-ins");
    // It is the last thing in the menu.
    expect(q("[data-agent-menu] [role=listbox]")!.lastElementChild!.contains(button)).toBe(true);
    await click(button);
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/connections"));
    await found("[data-agent-menu-group]");
  });

  it("lists every coding app with a switch: the leading four on, the others on only while one of their accounts works", async () => {
    await mountApp("/settings/connections", HOST);
    await found("[data-agent-menu-group]");
    const rows = () => qa("[data-agent-menu-row]").map((r) => r.dataset.agentMenuRow);
    await waitFor(() => expect(rows().slice(0, 4)).toEqual(["codex", "hermes", "claude-code", "opencode"]));
    expect(rows()).toHaveLength(9);
    for (const app of ["codex", "hermes", "claude-code", "opencode"]) expect((await sw(app)).getAttribute("aria-checked")).toBe("true");
    for (const app of ["gemini-cli", "antigravity", "goose", "pi", "openclaw"]) expect((await sw(app)).getAttribute("aria-checked")).toBe("false");
    expect((await sw("codex")).getAttribute("aria-label")).toBe("Show in the agent menu: OpenAI Codex");
    expect(q("[data-agent-menu-group]")!.textContent).not.toContain("·");
  });

  it("a hidden app leaves Home's menu, keeps its sign-ins, and the pick falls back to the first app shown", async () => {
    const { router } = await mountApp("/settings/connections", HOST);
    await click(await sw("codex"));
    await waitFor(() => expect(q('[data-agent-menu-switch="codex"]')!.getAttribute("aria-checked")).toBe("false"));
    await act(async () => {
      await router.navigate({ to: "/home/ask" });
    });
    await screen.findByLabelText("What should it do?");
    // Codex was the default pick; it is hidden, so the chip falls to the next connected app.
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toBe("Sonnet 5.5High"));
    await openMenu();
    expect(groups()).toEqual(["hermes", "claude-code", "opencode"]);
    // Showing it again puts it back at its place.
    await act(async () => {
      await router.navigate({ to: "/settings/connections" });
    });
    await click(await sw("codex"));
    await act(async () => {
      await router.navigate({ to: "/home/ask" });
    });
    await openMenu();
    await waitFor(() => expect(groups()).toEqual(["codex", "hermes", "claude-code", "opencode"]));
  });

  it("an app outside the leading four can be shown before it is signed in", async () => {
    const { router } = await mountApp("/settings/connections", HOST);
    await click(await sw("goose"));
    await waitFor(() => expect(q('[data-agent-menu-switch="goose"]')!.getAttribute("aria-checked")).toBe("true"));
    await act(async () => {
      await router.navigate({ to: "/home/ask" });
    });
    await openMenu();
    await waitFor(() => expect(groups()).toContain("goose"));
    expect(q('[data-agent-sign-in="goose"]')!.textContent).toBe("Sign in");
  });

  it("never hides the last app: its switch is off limits and the page says why", async () => {
    await mountApp("/settings/connections", HOST);
    for (const app of ["codex", "hermes", "claude-code"]) {
      await click(await sw(app));
      await waitFor(() => expect(q(`[data-agent-menu-switch="${app}"]`)!.getAttribute("aria-checked")).toBe("false"));
    }
    const last = await sw("opencode");
    expect(last.getAttribute("aria-checked")).toBe("true");
    expect(last.hasAttribute("disabled") || last.getAttribute("aria-disabled") === "true" || last.dataset.disabled !== undefined).toBe(true);
    await click(last);
    expect(last.getAttribute("aria-checked")).toBe("true");
    await waitFor(() => expect(q("[data-agent-menu-note]")!.textContent).toContain("OpenCode stays in the menu: at least one coding app has to be shown."));
    // Another app on lets it go.
    await click(await sw("goose"));
    await waitFor(() => expect(q('[data-agent-menu-switch="opencode"]')!.hasAttribute("disabled")).toBe(false));
  });
});
