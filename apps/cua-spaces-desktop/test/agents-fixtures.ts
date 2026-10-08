// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// An in-memory daemon and coding agents for the Agents tests (the SwiftUI
// app's FakeAgentsTools, FakeAgentKeysTools and FixtureAgentSetup): nothing
// on disk is read or written, and no key is ever stored.
import type { AgentSetupRunning, ToolRunner } from "../src/model/agents";
import { ToolError } from "../src/model/backend";
import type { AppAgentSetupOutcomeInput, AppAgentSetupStatus } from "../src/native/generated/index";

/** The runs `AgentRunsTests` reads: one running, one failed, one unreadable record. */
export const RUNS = [
  `{"run_id":"r-run","harness":"claude-code","status":"running","phase":"working","reason":"a turn is running","turn":1,"accepts_message":false,"meta":{"run_id":"r-run","harness":"claude-code","prompt":"fix\\n the build","cwd":"/root","created_at":20.0}}`,
  `{"run_id":"r-fail","harness":null,"status":"failed","phase":"failed","reason":"auth failed","turn":0,"accepts_message":false,"meta":{"run_id":"r-fail","harness":"openai-codex","prompt":"triage","cwd":"/root","created_at":10.0}}`,
  `{"run_id":"r-bad","status":"unknown","phase":"unknown","reason":"record unreadable","turn":0,"accepts_message":false}`,
];

export class FakeAgentsTools {
  calls: string[] = [];
  args: Record<string, unknown>[] = [];
  paused = false;
  grants: Record<string, unknown>[] = [];
  readonly now = 1_790_000_000_000;

  run: ToolRunner = async (tool, args) => {
    this.calls.push(tool);
    this.args.push(args);
    switch (tool) {
      case "persistent_agent_list":
        return {
          agents: [
            { name: "ada", harness: "hermes", space: "local:dev", paused: this.paused, run_id: this.paused ? null : "run-1", saved_ms: this.now - 120_000 },
            { name: "scout", harness: "claude-code", space: "cloud:scout", paused: true, space_state: "released", saved_ms: this.now - 7_200_000 },
          ],
        };
      case "agent_pause":
        this.paused = true;
        return {};
      case "agent_resume":
        this.paused = false;
        return {};
      case "volume_ls": {
        const path = args.path;
        if (path === "agents/ada/") return { entries: [{ path: "agents/ada/hermes/", name: "hermes", folder: true }] };
        if (path === "agents/ada/hermes/") return { entries: [{ path: "agents/ada/hermes/MEMORY.md", name: "MEMORY.md", folder: false, size: 1830 }] };
        return { entries: [] };
      }
      case "routine_list":
        return { routines: [{ id: "R1", botID: "ada", title: "Morning", label: "Every day at 8:00 AM", isEnabled: true }] };
      case "computer_access_list":
        return { grants: this.grants, audit: [] };
      case "computer_access_grant":
        this.grants = [{ agent: "ada", machine: args.machine ?? "", revoked: false }];
        return this.grants[0];
      case "agent_events":
        return { run_id: args.run_id, status: "idle", phase: "idle", events: [], cursor: args.cursor, caught_up: true };
      default:
        throw new ToolError(`unknown tool ${tool}`);
    }
  };
}

/** `agent_keys.*` as the daemon answers them (never a value). */
export class FakeAgentKeysTools {
  calls: [string, Record<string, unknown>][] = [];
  keys: Record<string, unknown>[] = [{ provider: "openai", env: "OPENAI_API_KEY", last4: "3f9a", added_ms: 1_790_000_000_000 }];
  available = true;

  run: ToolRunner = async (tool, args) => {
    this.calls.push([tool, args]);
    switch (tool) {
      case "agent_keys.list":
        break;
      case "agent_keys.set": {
        const provider = String(args.provider ?? "");
        const env = provider === "anthropic" ? "ANTHROPIC_API_KEY" : provider === "openai" ? "OPENAI_API_KEY" : String(args.env ?? "");
        const value = String(args.value ?? "");
        this.keys = this.keys.filter((k) => k.env !== env);
        this.keys.push({ provider, env, last4: value.slice(-4), added_ms: 1_790_000_100_000 });
        break;
      }
      case "agent_keys.remove":
        this.keys = this.keys.filter((k) => k.env !== args.env);
        break;
      default:
        throw new ToolError(`unknown tool ${tool}`);
    }
    const report: Record<string, unknown> = { keys: this.keys, providers: [], available: this.available };
    if (!this.available) report.unavailable = "this build keeps credentials in a file";
    return report;
  };
}

/** In-memory coding agents: nothing on disk is read or written. */
export class FixtureAgentSetup implements AgentSetupRunning {
  current: AppAgentSetupStatus[];
  calls: string[] = [];

  constructor(statuses?: AppAgentSetupStatus[]) {
    this.current = statuses ?? [
      { id: "claude-code", name: "Claude Code", installed: true, skillsDir: "~/.claude/skills", mcpConfig: "~/.claude.json", cuaConfigured: false, skillsInstalled: [], skillsOutdated: [], error: undefined },
      { id: "codex", name: "Codex", installed: true, skillsDir: "~/.agents/skills", mcpConfig: "~/.codex/config.toml", cuaConfigured: true, skillsInstalled: ["cua-spaces", "cua-sandbox"], skillsOutdated: [], error: undefined },
      { id: "cursor", name: "Cursor", installed: false, skillsDir: undefined, mcpConfig: undefined, cuaConfigured: false, skillsInstalled: [], skillsOutdated: [], error: undefined },
    ];
  }

  async statuses() {
    return this.current;
  }

  skillsTotal() {
    return 2;
  }

  async setUp(agents: string[], skills: boolean, mcp: boolean) {
    this.calls.push(`setup:${agents.join(",")}`);
    const out: AppAgentSetupOutcomeInput[] = [];
    for (const a of this.current.filter((s) => agents.includes(s.id))) {
      if (skills && a.skillsDir !== undefined) {
        a.skillsInstalled = ["cua-spaces", "cua-sandbox"];
        out.push({ agents: [a.id], target: "skill", item: "cua-spaces", change: "created", detail: "" });
      }
      if (mcp && a.mcpConfig !== undefined) {
        a.cuaConfigured = true;
        out.push({ agents: [a.id], target: "mcp", item: "cua", change: "created", detail: "" });
      }
    }
    return out;
  }

  async setUpCuaDriver(agents: string[]) {
    this.calls.push(`driver:${agents.join(",")}`);
    const out: AppAgentSetupOutcomeInput[] = [];
    for (const a of this.current.filter((s) => agents.includes(s.id))) {
      if (a.skillsDir !== undefined) out.push({ agents: [a.id], target: "skill", item: "cua-driver", change: "created", detail: "" });
      if (a.mcpConfig !== undefined) out.push({ agents: [a.id], target: "mcp", item: "cua-driver", change: "created", detail: "" });
    }
    return out;
  }

  async remove(agents: string[]) {
    this.calls.push(`remove:${agents.join(",")}`);
    for (const a of this.current.filter((s) => agents.includes(s.id))) {
      a.skillsInstalled = [];
      a.cuaConfigured = false;
    }
    return [];
  }
}

/** The methods test/agents.test.ts checks (their answers need a daemon's tools, which the shared bridge test has none of). */
export const AGENT_METHODS = [
  "agents.list", "agents.runs", "agents.events", "agents.pause", "agents.resume", "agents.setup", "agents.configure", "agents.setupDriver",
  "agentKeys.list", "agentKeys.set", "agentKeys.remove",
];
