// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Right-click menus on the Agents page and the Keyvault list (CUA-1639):
// each offers its page's own actions by the same rules, runs them through
// the same bridge calls, asks before Remove and Delete, and never puts an
// item's name or value in a label.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { BridgeProvider, type AgentSummary, type DataAdapter, type VaultRow, type WkContextMenuRequest } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { agentMenuEntries } from "@/components/agent-context-menu";
import { ContextMenuHost, normalizeMenu, toWireItems, useContextMenuStore, type ContextMenuEntry } from "@/components/context-menu";
import { KeyvaultPage } from "@/components/keyvault/keyvault-page";
import { vaultItemMenuEntries } from "@/components/keyvault/vault-list";
import { ToastProvider } from "@/components/ui/toast";
import { mountApp, stubBrowserApis } from "@/__tests__/app-harness";

// The virtualized list measures its viewport, which jsdom has none of: draw every row.
vi.mock("@legendapp/list/react", () => ({
  LegendList: ({ data, renderItem, keyExtractor }: { data: unknown[]; renderItem: (a: { item: unknown }) => React.ReactNode; keyExtractor: (i: unknown) => string }) => (
    <div>{data.map((item) => <div key={keyExtractor(item)}>{renderItem({ item })}</div>)}</div>
  ),
}));

afterEach(() => {
  cleanup();
  useContextMenuStore.setState({ open: null, confirming: null });
});
stubBrowserApis();

const ids = (entries: ContextMenuEntry[]) => toWireItems(normalizeMenu(entries)).map((i) => (i.separator ? "-" : `${i.id}${i.enabled === false ? " (off)" : ""}`));

const menuOf = async (shown: WkContextMenuRequest[], el: Element) => {
  const before = shown.length;
  expect(fireEvent.contextMenu(el, { clientX: 10, clientY: 10, button: 2 })).toBe(false);
  await waitFor(() => expect(shown.length).toBe(before + 1));
  return shown.at(-1)!.items.map((i) => (i.separator ? "-" : `${i.id}${i.enabled === false ? " (off)" : ""}`));
};

const find = (selector: string) =>
  waitFor(() => {
    const el = document.querySelector<HTMLElement>(selector);
    if (!el) throw new Error(`no ${selector}`);
    return el;
  });

/* ---- Agents ---------------------------------------------------------------- */

describe("an agent's menu (the items)", () => {
  const noop = { open: () => {}, pause: async () => {}, resume: async () => {}, stop: async () => {}, remove: async () => {} };
  const agent = (patch: Partial<AgentSummary>): AgentSummary =>
    ({
      name: "ada",
      harness: "claude-code",
      harnessName: "Claude Code",
      spaceId: "local:dev",
      spaceName: "dev",
      spaceState: "running",
      state: "running",
      detail: "",
      actionLabel: "Pause",
      lastActivityMs: 0,
      run: null,
      lastError: null,
      record: { name: "ada", harness: "claude-code", space: "local:dev", paused: false, spaceState: "running", runId: "run-1", savedMs: 0 },
      ...patch,
    }) as AgentSummary;

  it("pauses and stops a running agent, resumes a paused one, and always asks before Remove", () => {
    expect(ids(agentMenuEntries(agent({}), noop))).toEqual(["agent.open", "-", "agent.pause", "agent.stop", "-", "agent.copyName", "-", "agent.remove"]);
    const paused = agent({ state: "paused", actionLabel: "Resume", record: { ...agent({}).record, paused: true, runId: null } });
    expect(ids(agentMenuEntries(paused, noop))).toEqual(["agent.open", "-", "agent.resume", "-", "agent.copyName", "-", "agent.remove"]);
    // Idle (a run waiting for a follow-up): nothing to stop.
    expect(ids(agentMenuEntries(agent({ state: "idle" }), noop))).not.toContain("agent.stop");
    // Its own detail: no Open.
    expect(ids(agentMenuEntries(agent({}), noop, { open: false }))[0]).toBe("agent.pause");
    const remove = normalizeMenu(agentMenuEntries(agent({}), noop)).find((e) => "id" in e && e.id === "agent.remove");
    expect(remove).toMatchObject({ destructive: true, confirm: { title: "Remove ada?", confirmLabel: "Remove" } });
  });
});

describe.skipIf(!wasmBuilt)("the Agents page's menus", () => {
  it("offer each agent's own actions by its state, and Open selects it", async () => {
    const shown: WkContextMenuRequest[] = [];
    let answer: string | null = null;
    await mountApp("/agents", { failsCreates: false, listsGhost: false, showContextMenu: async (m) => (shown.push(m), answer) });
    const ada = await find('[data-agent-row="ada"]');
    expect(await menuOf(shown, ada)).toEqual(["agent.open", "-", "agent.pause", "agent.stop", "-", "agent.copyName", "-", "agent.remove"]);
    expect(shown.at(-1)!.items.find((i) => i.id === "agent.copyName")?.copy).toBe("ada");
    expect(await menuOf(shown, await find('[data-agent-row="scout"]'))).toEqual(["agent.open", "-", "agent.resume", "-", "agent.copyName", "-", "agent.remove"]);
    expect(await menuOf(shown, await find('[data-agent-row="atlas"]'))).not.toContain("agent.stop");

    answer = "agent.open";
    const atlas = await find('[data-agent-row="atlas"]');
    fireEvent.contextMenu(atlas, { clientX: 10, clientY: 10, button: 2 });
    await waitFor(() => expect(document.querySelector('[data-agent-row="atlas"]')?.getAttribute("aria-current")).toBe("true"));
    // The detail's header has the same menu, without Open.
    answer = null;
    expect(await menuOf(shown, await find('[data-agent-header="atlas"]'))).toEqual(["agent.pause", "-", "agent.copyName", "-", "agent.remove"]);
  });

  it("stops a running agent's run with agents.stop", async () => {
    const calls: [string, unknown][] = [];
    await mountApp("/agents", { failsCreates: false, listsGhost: false, showContextMenu: async () => "agent.stop", intercept: (op, args) => void calls.push([op, args]) });
    fireEvent.contextMenu(await find('[data-agent-row="ada"]'), { clientX: 10, clientY: 10, button: 2 });
    await waitFor(() => expect(calls.find(([op]) => op === "agents.stop")?.[1]).toEqual({ spaceId: "local:design-review", runId: "run-ada-7" }));
  });

  it("removes an agent only after the confirmation", async () => {
    const calls: [string, unknown][] = [];
    await mountApp("/agents", { failsCreates: false, listsGhost: false, showContextMenu: async () => "agent.remove", intercept: (op, args) => void calls.push([op, args]) });
    fireEvent.contextMenu(await find('[data-agent-row="atlas"]'), { clientX: 10, clientY: 10, button: 2 });
    const confirm = await screen.findByRole("button", { name: "Remove" });
    expect(screen.getByText("Remove atlas?")).toBeTruthy();
    expect(calls.some(([op]) => op === "agents.remove")).toBe(false);
    fireEvent.click(confirm);
    await waitFor(() => expect(calls.find(([op]) => op === "agents.remove")?.[1]).toEqual({ name: "atlas" }));
    await waitFor(() => expect(document.querySelector('[data-agent-row="atlas"]')).toBeNull());
  });

  it("a run's row offers Open, and Stop only while it runs", async () => {
    const shown: WkContextMenuRequest[] = [];
    await mountApp("/agents", { failsCreates: false, listsGhost: false, showContextMenu: async (m) => (shown.push(m), null) });
    expect(await menuOf(shown, await find('[data-run-row="run-9b20"]'))).toEqual(["run.open"]);
    expect(await menuOf(shown, await find('[data-run-row="run-ada-7"]'))).toEqual(["run.open", "-", "run.stop"]);
  });
});

/* ---- Keyvault -------------------------------------------------------------- */

describe("a Keyvault item's menu (the items)", () => {
  const actions = { lock: () => {}, unlock: () => {} };
  const row = (patch: Partial<VaultRow>): VaultRow => ({
    id: "kv-1",
    kind: "cookie",
    kindLabel: "Cookie",
    symbol: "",
    title: "session",
    subtitle: "linear.app",
    updated: "",
    locked: true,
    lockSymbol: "",
    lockHelp: "",
    identityProvider: false,
    selected: false,
    ...patch,
  });
  const base = { namesHidden: false, blocked: false, actions, onDelete: () => {} };

  it("copies the name (never while names are hidden), flips the lock as the row's button does, and deletes", () => {
    expect(ids(vaultItemMenuEntries(row({}), base))).toEqual(["keyvault.copyName", "-", "keyvault.unlock", "-", "keyvault.delete"]);
    expect(ids(vaultItemMenuEntries(row({ locked: false }), base))).toContain("keyvault.lock");
    // An identity provider's session always asks: no Unlock.
    expect(ids(vaultItemMenuEntries(row({ identityProvider: true }), base))).toEqual(["keyvault.copyName", "-", "keyvault.delete"]);
    expect(ids(vaultItemMenuEntries(row({}), { ...base, namesHidden: true }))).toEqual(["keyvault.unlock", "-", "keyvault.delete"]);
    expect(ids(vaultItemMenuEntries(row({}), { ...base, blocked: true }))).toContain("keyvault.unlock (off)");
  });

  it("never names the item in a label", () => {
    const labels = toWireItems(normalizeMenu(vaultItemMenuEntries(row({ title: "user_session", subtitle: "github.com" }), base))).map((i) => i.label);
    expect(labels.filter(Boolean)).toEqual(["Copy Name", "Unlock…", "Delete…"]);
  });
});

describe.skipIf(!wasmBuilt)("the Keyvault list's item menu", () => {
  async function mount(choose: (m: WkContextMenuRequest) => string | null) {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 2 });
    const shown: WkContextMenuRequest[] = [];
    const calls: [string, unknown][] = [];
    const adapter: DataAdapter = {
      mode: demo.mode,
      call: ((op: string, args: unknown) => {
        calls.push([op, args]);
        return demo.call(op as never, args as never);
      }) as DataAdapter["call"],
      subscribe: demo.subscribe.bind(demo),
      showContextMenu: async (m) => {
        shown.push(m);
        return choose(m);
      },
    };
    render(
      <BridgeProvider adapter={adapter} core={await testCore()}>
        <ToastProvider>
          <KeyvaultPage />
          <ContextMenuHost />
        </ToastProvider>
      </BridgeProvider>,
    );
    // Open linear.app (under Google Chrome, which starts open).
    fireEvent.click(await find('[data-vault-group="linear.app"]'));
    return { shown, calls, item: await find('[data-vault-item="kv-linear"]') };
  }

  it("offers Copy Name, Unlock and Delete, and copies the name, not a value", async () => {
    const { shown, item } = await mount(() => null);
    expect(await menuOf(shown, item)).toEqual(["keyvault.copyName", "-", "keyvault.unlock", "-", "keyvault.delete"]);
    expect(shown.at(-1)!.items.find((i) => i.id === "keyvault.copyName")?.copy).toBe("session");
  });

  it("deletes an item only after the page's confirmation", async () => {
    const { calls, item } = await mount((m) => (m.items.some((i) => i.id === "keyvault.delete") ? "keyvault.delete" : null));
    fireEvent.contextMenu(item, { clientX: 10, clientY: 10, button: 2 });
    const confirm = await screen.findByRole("button", { name: "Delete" });
    expect(calls.some(([op]) => op === "keyvault.delete")).toBe(false);
    fireEvent.click(confirm);
    await waitFor(() => expect(calls.find(([op]) => op === "keyvault.delete")?.[1]).toEqual({ itemIds: ["kv-linear"] }));
    await waitFor(() => expect(document.querySelector('[data-vault-item="kv-linear"]')).toBeNull());
  });
});
