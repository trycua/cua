// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Setup's agents step after Continue (CUA-4067): one row per thing Cua does, live, each saying what and where in
// plain words; a row that fails says why and retries on its own; Cua Driver is one of the rows; this computer is set
// up to run jobs first.

import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import type { AgentSetupRow, ConnectResult } from "@/bridge/contracts/agents";
import { writeFirstRun } from "@/bridge/first-run";
import { ChecklistScreen } from "@/components/first-run-new/first-run-new";
import {
  CHECKLIST_WAITING,
  checklistFromResult,
  checklistKey,
  checklistRowView,
  checklistSettled,
  checklistView,
  connectArgs,
  connectedAgents,
  connectPlan,
  type ChecklistState,
} from "@/lib/first-run-new";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
afterEach(cleanup);

const done = (where: string | null = null): ChecklistState => ({ status: "done", where, line: null });
const result = (parts: ConnectResult["parts"], extra: Partial<ConnectResult> = {}): ConnectResult => ({
  agent: "claude-code",
  tools_ok: true,
  tools_line: null,
  sign_in: null,
  parts,
  ...extra,
});

describe("the plan", () => {
  it("sets this computer up first, then each agent's tools, skills, Cua Driver and sign-in", () => {
    const plan = connectPlan({ agents: ["claude-code", "gemini-cli"], driver: true, thisComputer: true });
    expect(plan.map((i) => i.key)).toEqual([
      "this-computer",
      "claude-code:mcp",
      "claude-code:skills",
      "claude-code:driver",
      "claude-code:sign_in",
      "gemini-cli:mcp",
      "gemini-cli:skills",
      "gemini-cli:driver",
    ]);
    // Signed out, away from a Mac: no row for this computer, no Cua Driver.
    expect(connectPlan({ agents: ["codex"], driver: false, thisComputer: false }).map((i) => i.part)).toEqual(["mcp", "skills", "sign_in"]);
  });

  it("runs each row as its own connect, so Retry runs only that row", () => {
    expect(connectArgs("codex", "mcp")).toEqual({ agents: ["codex"], signIn: false, skills: false, mcp: true });
    expect(connectArgs("codex", "skills")).toEqual({ agents: ["codex"], signIn: false, skills: true, mcp: false });
    expect(connectArgs("codex", "driver")).toEqual({ agents: ["codex"], signIn: false, skills: false, mcp: false, driver: true });
    expect(connectArgs("codex", "sign_in").signIn).toBe(true);
  });
});

describe("a row from the host's answer", () => {
  it("takes the part's own status, where and why", () => {
    const r = result([
      { part: "mcp", status: "failed", where: "~/.claude.json", line: "Couldn't write ~/.claude.json: Permission denied" },
      { part: "driver", status: "needs_permission", where: "~/.claude.json", line: "Allow Screen Recording and Accessibility." },
    ]);
    expect(checklistFromResult("mcp", r)).toEqual({ status: "failed", where: "~/.claude.json", line: "Couldn't write ~/.claude.json: Permission denied" });
    expect(checklistFromResult("driver", r)?.status).toBe("needs_permission");
    // The host answered parts but none for this one (an app with no Cua Driver): no row.
    expect(checklistFromResult("skills", r)).toBeNull();
    expect(checklistFromResult("mcp", undefined)?.status).toBe("failed");
  });

  it("makes Codex's code sign-in a step to do, not a failure", () => {
    const r = result([{ part: "sign_in", status: "failed", where: null, line: "Codex signs in with a code." }], {
      agent: "codex",
      sign_in: { imported: null, refused: "not_yet", line: "Codex signs in with a code." },
    });
    expect(checklistFromResult("sign_in", r)?.status).toBe("needs_code");
  });

  it("reads an older host's single answer", () => {
    const old = result(undefined, { tools_ok: false, tools_line: "Couldn't add Cua to ~/.claude.json" });
    expect(checklistFromResult("mcp", old)).toEqual({ status: "failed", where: null, line: "Couldn't add Cua to ~/.claude.json" });
    expect(checklistFromResult("driver", old)).toBeNull();
  });
});

describe("a row's words", () => {
  const item = (part: "mcp" | "skills" | "driver" | "sign_in", agent = "claude-code") => ({ key: checklistKey(agent, part), agent, part });

  it("say what is done and where, in plain words", () => {
    expect(checklistRowView(item("mcp"), done("~/.claude.json"), "macos").text).toBe("Added Cua's tools to Claude Code (~/.claude.json)");
    expect(checklistRowView(item("skills"), done("~/.claude/skills"), "macos").text).toBe("Installed Cua's skills for Claude Code (~/.claude/skills)");
    expect(checklistRowView(item("driver"), done("~/.claude.json"), "macos").text).toBe("Let Claude Code use apps on this Mac (Cua Driver)");
    expect(checklistRowView(item("sign_in"), { status: "running", where: null, line: null }, "macos").text).toBe(
      "Saving your Claude Code sign-in to Keyvault so jobs on your other computers can use it",
    );
    const mac = checklistRowView({ key: "this-computer", agent: null, part: "this-computer" }, done(), "macos");
    expect(mac.text).toBe("This Mac can run your agents' jobs");
    expect(mac.sub).toBe("Shares this Mac's desktop with Cua so jobs can run on it. You can turn this off in Settings.");
    for (const v of [mac, checklistRowView(item("mcp"), done("~/.claude.json"), "macos")])
      for (const word of ["MCP", "relay", "host", "config", "cua-driver"]) expect(`${v.text} ${v.sub ?? ""}`).not.toContain(word);
  });

  it("a failed row says why and offers Retry; a driver without its permissions offers Allow", () => {
    const failed = checklistRowView(item("mcp"), { status: "failed", where: null, line: "Couldn't write ~/.claude.json: Permission denied" }, "macos");
    expect(failed.text).toBe("Couldn't add Cua's tools to Claude Code");
    expect(failed.sub).toBe("Couldn't write ~/.claude.json: Permission denied");
    expect(failed.action).toEqual({ id: "retry", label: "Retry" });
    const perm = checklistRowView(item("driver"), { status: "needs_permission", where: null, line: null }, "macos");
    expect(perm.action).toEqual({ id: "allow", label: "Allow" });
    expect(perm.sub).toContain("Screen Recording and Accessibility");
    expect(checklistRowView(item("mcp"), done(), "macos").action).toBeNull();
    const code = checklistRowView(item("sign_in", "codex"), { status: "needs_code", where: null, line: null }, "macos");
    expect(code.text).toBe("Sign Codex in for your other computers");
    expect(code.sub).toBe("Codex signs in with a code made for Cua. Your own sign-in stays as it is.");
    expect(code.action).toEqual({ id: "code", label: "Get a code" });
    const waiting = checklistRowView(
      item("sign_in", "codex"),
      { status: "needs_code", where: null, line: null, code: { code: "QX7P", url: "https://x" } },
      "macos",
    );
    expect(waiting.sub).toContain("QX7P");
  });

  it("the heading follows the rows; Ready names the agents whose tools went in", () => {
    const fail: ChecklistState = { status: "failed", where: null, line: "x" };
    expect(checklistSettled([done(), CHECKLIST_WAITING])).toBe(false);
    expect(checklistView([done(), { status: "running", where: null, line: null }]).title).toBe("Connecting your agents");
    expect(checklistView([done(), done()]).title).toBe("Your agents are connected");
    expect(checklistView([done(), fail]).title).toBe("Almost there");
    expect(connectedAgents(["claude-code", "codex"], { "claude-code:mcp": done(), "codex:mcp": fail })).toEqual(["claude-code"]);
  });
});

describe("the screen", () => {
  it("draws every row with its mark, hides skipped ones, and retries only the row asked", () => {
    const onAction = vi.fn();
    const plan = connectPlan({ agents: ["claude-code"], driver: true, thisComputer: true });
    const states: Record<string, ChecklistState> = {
      "this-computer": done(),
      "claude-code:mcp": { status: "failed", where: "~/.claude.json", line: "Couldn't write ~/.claude.json: Permission denied" },
      "claude-code:skills": { status: "running", where: null, line: null },
      "claude-code:driver": { status: "skipped", where: null, line: null },
      "claude-code:sign_in": CHECKLIST_WAITING,
    };
    render(
      <ChecklistScreen
        view={checklistView(Object.values(states))}
        rows={plan.map((i) => checklistRowView(i, states[i.key]!, "macos"))}
        onAction={onAction}
        onContinue={null}
      />,
    );
    const rows = [...document.querySelectorAll<HTMLElement>("[data-checklist-row]")];
    expect(rows.map((r) => r.dataset.checklistRow)).toEqual(["this-computer", "claude-code:mcp", "claude-code:skills", "claude-code:sign_in"]);
    expect(rows.map((r) => r.dataset.status)).toEqual(["done", "failed", "running", "waiting"]);
    expect(document.querySelectorAll("[data-checklist-action]").length).toBe(1);
    fireEvent.click(document.querySelector("[data-checklist-row='claude-code:mcp'] [data-checklist-action=retry]")!);
    expect(onAction).toHaveBeenCalledWith("claude-code:mcp", "retry");
    // Nothing to continue to while a row runs.
    expect(document.querySelector<HTMLButtonElement>("[data-onboarding-primary]")!.disabled).toBe(true);
    // No coloured edge on any row: the mark and the words carry it.
    for (const r of rows) expect(r.className).not.toMatch(/border-l/);
  });
});

const heading = () => document.querySelector("[data-onboarding-heading]")?.textContent ?? "";
const onlyClaude = (rows: AgentSetupRow[]) => rows.map((r) => ({ ...r, installed: r.agent === "claude-code" }));
type Calls = { op: string; args: unknown }[];
const status = (key: string) => document.querySelector(`[data-checklist-row='${key}']`)?.getAttribute("data-status");

describe.skipIf(!wasmBuilt)("the checklist on its route", () => {
  it("signed in: sets this Mac up and waits for it before connecting the agent", { timeout: 60_000 }, async () => {
    const calls: Calls = [];
    await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: true, onboarded: false, noSpaces: true },
      intercept: (op, args) => (calls.push({ op, args }), undefined),
      answer: (op, out) => (op === "agents.setup" ? onlyClaude(out as AgentSetupRow[]) : out),
    });
    writeFirstRun({ newRun: { screen: "agent", reachedStep: "agents" } });
    await waitFor(() => expect(heading()).toBe("Which agents do you use?"), { timeout: 15_000 });
    fireEvent.click(document.querySelector("[data-onboarding-primary]")!);
    await waitFor(() => expect(status("this-computer")).toBe("done"), { timeout: 20_000 });
    expect(document.querySelector("[data-checklist-row='this-computer'] [data-checklist-text]")!.textContent).toBe("This Mac can run your agents' jobs");
    await waitFor(() => expect(status("claude-code:sign_in")).toBe("done"), { timeout: 20_000 });
    // This Mac was set up (its desktop shared, which restarts its service) before any agent was connected.
    const firstConnect = calls.findIndex((c) => c.op === "agents.connect");
    const lastHostStatus = calls.map((c) => c.op).lastIndexOf("host.status", firstConnect);
    expect(lastHostStatus).toBeGreaterThan(-1);
    expect(calls.slice(0, firstConnect).some((c) => c.op === "host.action" || c.op === "host.setUp" || c.op === "machines.register")).toBe(true);
  });

  it("a row that fails says why, and its Retry runs only that row", { timeout: 60_000 }, async () => {
    const calls: Calls = [];
    let fail = true;
    await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: false, onboarded: false, noSpaces: true },
      intercept: (op, args) => {
        calls.push({ op, args });
        const a = args as { mcp?: boolean; skills?: boolean };
        if (op === "agents.connect" && a.mcp === true && a.skills === false && fail) {
          fail = false;
          return Promise.reject(new Error("Couldn't write ~/.claude.json: Permission denied (os error 13)"));
        }
        return undefined;
      },
      answer: (op, out) => (op === "agents.setup" ? onlyClaude(out as AgentSetupRow[]) : out),
    });
    writeFirstRun({ newRun: { screen: "agent", reachedStep: "agents" } });
    await waitFor(() => expect(heading()).toBe("Which agents do you use?"), { timeout: 15_000 });
    fireEvent.click(document.querySelector("[data-onboarding-primary]")!);
    await waitFor(() => expect(status("claude-code:sign_in")).toBe("done"), { timeout: 20_000 });
    expect(status("claude-code:mcp")).toBe("failed");
    expect(document.querySelector("[data-checklist-row='claude-code:mcp'] [data-checklist-sub]")!.textContent).toContain("Permission denied");
    // The rest went on without it.
    expect(status("claude-code:skills")).toBe("done");
    const before = calls.filter((c) => c.op === "agents.connect").length;
    fireEvent.click(document.querySelector("[data-checklist-row='claude-code:mcp'] [data-checklist-action=retry]")!);
    await waitFor(() => expect(status("claude-code:mcp")).toBe("done"), { timeout: 15_000 });
    const after = calls.filter((c) => c.op === "agents.connect").slice(before);
    expect(after.map((c) => c.args)).toEqual([{ agents: ["claude-code"], signIn: false, skills: false, mcp: true }]);
  });
});
