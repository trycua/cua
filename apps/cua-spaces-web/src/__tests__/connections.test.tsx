// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The agents on This Mac (CUA-2268): every connection state the status can
// report becomes one label and at most one button, the same in the core
// (`agents.connectionRows`) and the page's fallback, and the Agents page
// draws them and runs the buttons through the host.

import { cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { connectionRows, type ConnectionRow } from "@/bridge";
import { noCore, testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const base: ConnectionRow = {
  id: "codex",
  name: "Codex",
  installed: true,
  version: "0.61.0",
  previous_version: "0.61.0",
  signed_in: "yes",
  sign_in: "ChatGPT sign-in",
  mcp: "ok",
  mcp_config: "~/.codex/config.toml",
  signin_imported: true,
  signin_import: "ok",
  state: "connected",
  problem: null,
  checked_at_ms: 1,
};

/** One row per cell of the matrix the status reports (the Rust matrix test's states). */
const MATRIX: Record<string, ConnectionRow> = {
  connected: base,
  upgraded: { ...base, previous_version: "0.60.1" },
  "not connected": { ...base, mcp: "missing", state: "not_connected", signin_imported: false, signin_import: "missing" },
  "not signed in": { ...base, signed_in: "no", state: "attention", problem: { line: "Connected. No ChatGPT sign-in on This Mac yet.", action: "sign_in" } },
  "sign-in expired": { ...base, signed_in: "expired", state: "attention", problem: { line: "Codex's ChatGPT sign-in expired. Sign in again.", action: "sign_in" } },
  "copy expired": { ...base, signin_import: "expired", state: "attention", problem: { line: "The copy of Codex's sign-in in Keyvault expired. Reconnect copies it again.", action: "reconnect" } },
  "entry broken": { ...base, mcp: "broken", state: "attention", problem: { line: "Cua's entry in ~/.codex/config.toml points at a cua that isn't there. Reconnect fixes it.", action: "reconnect" } },
  "entry removed": { ...base, mcp: "missing", state: "attention", problem: { line: "Cua was taken out of ~/.codex/config.toml. Reconnect adds it back.", action: "reconnect" } },
  "other cua": { ...base, mcp: "stale", problem: { line: "Codex uses another copy of cua. Reconnect to use this one.", action: "reconnect" } },
  uninstalled: { ...base, installed: false, version: null, state: "attention", problem: { line: "Codex isn't on This Mac anymore. Disconnect takes Cua out of its settings.", action: "disconnect" } },
  "no key yet": { ...base, id: "hermes", name: "Hermes", signed_in: "no", sign_in: "provider key", signin_import: "missing", signin_imported: false, state: "attention", problem: { line: "Connected. No provider key on This Mac yet.", action: "none" } },
  "unknown sign-in": { ...base, id: "cursor", name: "Cursor", signed_in: "unknown", sign_in: null, signin_import: "unsupported", signin_imported: false },
  "not installed": { ...base, installed: false, mcp: "missing", state: "not_installed" },
};

const WANT: Record<string, [label: string, action: string | null, problem: boolean]> = {
  connected: ["Connected", null, false],
  upgraded: ["Connected", null, false],
  "not connected": ["Not connected", "connect", false],
  "not signed in": ["Connected. No ChatGPT sign-in on This Mac yet.", "sign-in", true],
  "sign-in expired": ["Codex's ChatGPT sign-in expired. Sign in again.", "sign-in", true],
  "copy expired": ["The copy of Codex's sign-in in Keyvault expired. Reconnect copies it again.", "reconnect", true],
  "entry broken": ["Cua's entry in ~/.codex/config.toml points at a cua that isn't there. Reconnect fixes it.", "reconnect", true],
  "entry removed": ["Cua was taken out of ~/.codex/config.toml. Reconnect adds it back.", "reconnect", true],
  "other cua": ["Connected", "reconnect", false],
  uninstalled: ["Codex isn't on This Mac anymore. Disconnect takes Cua out of its settings.", "disconnect", true],
  "no key yet": ["Connected. No provider key on This Mac yet.", null, true],
  "unknown sign-in": ["Connected", null, false],
};

describe("connection rows", () => {
  it("every state has one label and at most one button (page fallback)", () => {
    for (const [cell, row] of Object.entries(MATRIX)) {
      const views = connectionRows(noCore, [row]);
      if (cell === "not installed") {
        expect(views).toEqual([]);
        continue;
      }
      const [label, action, problem] = WANT[cell]!;
      expect([views[0]!.label, views[0]!.action?.id ?? null, views[0]!.problem], cell).toEqual([label, action, problem]);
      // Never "Connected" for a broken row, never "Not connected" for a working one.
      if (row.state !== "connected") expect(views[0]!.label, cell).not.toBe("Connected");
      if (row.mcp === "ok" || row.mcp === "stale") expect(views[0]!.label, cell).not.toBe("Not connected");
    }
    expect(connectionRows(noCore, [MATRIX.upgraded!])[0]!.detail).toBe("0.61.0, updated from 0.60.1");
    expect(connectionRows(noCore, [base])[0]!.signIn).toBe("ChatGPT sign-in in Keyvault");
  });

  it.skipIf(!wasmBuilt)("the core draws the same rows as the fallback", async () => {
    const core = await testCore();
    const rows = Object.values(MATRIX);
    expect(connectionRows(core, rows)).toEqual(connectionRows(noCore, rows));
  });
});

describe.skipIf(!wasmBuilt)("the Agents page, On This Mac", () => {
  it("lists the agents with their state; Connect opens Connectors at that app instead of connecting", { timeout: 20_000 }, async () => {
    const { router } = await mountApp("/agents/all", { failsCreates: false, listsGhost: false });
    const row = (id: string) => document.querySelector<HTMLElement>(`[data-connection-row="${id}"]`);
    await waitFor(() => expect(row("hermes")).not.toBeNull(), { timeout: 8000 });
    // The demo: Claude Code connected, Hermes not, Codex connected without its sign-in.
    expect(row("claude-code")!.getAttribute("data-connection-state")).toBe("connected");
    expect(row("hermes")!.getAttribute("data-connection-state")).toBe("not_connected");
    expect(row("codex")!.textContent).toContain("Connected. No ChatGPT sign-in on This Mac yet.");
    expect(row("codex")!.querySelector("[data-connection-action]")!.textContent).toBe("Sign in");
    // Not installed agents get no row.
    expect(row("gemini-cli")).toBeNull();
    // Connect on Hermes: opens Settings, Connectors at its row; nothing is connected on the spot.
    fireEvent.click(row("hermes")!.querySelector('[data-connection-action="connect"]')!);
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/connectors"));
    expect((router.state.location.search as { app?: string }).app).toBe("hermes");
    const appRow = await waitFor(() => {
      const r = document.querySelector<HTMLElement>('[data-connected-app="hermes"]');
      expect(r).not.toBeNull();
      return r!;
    }, { timeout: 8000 });
    expect(appRow.dataset.on).toBe("false");
  });
});
