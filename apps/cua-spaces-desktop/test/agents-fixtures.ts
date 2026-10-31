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
      case "computer_access_revoke":
        this.grants = [];
        return {};
      case "volume_read":
        return { path: args.path, content: "# Ada\n", encoding: "utf8" };
      case "volume_history":
        return {
          versions: [
            { version: "v2", modified_ms: this.now - 60_000, deleted: false, latest: true },
            { version: "v1", modified_ms: this.now - 3_600_000, deleted: false, latest: false },
          ],
        };
      case "volume_restore":
      case "routine_add":
      case "routine_set_enabled":
      case "routine_remove":
        return {};
      case "agent_start":
        return { run_id: "run-2", agent: args.agent, space: args.space, process_tag: "cua-agent-run-2", cwd: "/home/cua/run-2", notes: [], hint: "" };
      case "agent_message":
        return { delivered: true, run_id: args.run_id, status: "running", phase: "working", reason: "" };
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
  uses: Record<string, Record<string, unknown>> = {};
  available = true;

  run: ToolRunner = async (tool, args) => {
    this.calls.push([tool, args]);
    switch (tool) {
      case "agent_keys.list":
      case "agent_keys.check":
        break;
      case "agent_keys.set": {
        const provider = String(args.provider ?? "");
        const env = provider === "anthropic" ? "ANTHROPIC_API_KEY" : provider === "openai" ? "OPENAI_API_KEY" : String(args.env ?? "");
        const value = String(args.value ?? "");
        // Another key beside the first one (named); saving the first again replaces only it.
        const slot = args.add === true && this.keys.some((k) => k.env === env && !k.slot) ? `k${this.keys.length}` : undefined;
        if (!slot) this.keys = this.keys.filter((k) => !(k.env === env && !k.slot));
        this.keys.push({
          provider,
          env,
          ...(slot ? { slot } : {}),
          ...(typeof args.label === "string" ? { label: args.label } : {}),
          last4: value.slice(-4),
          added_ms: 1_790_000_100_000,
          ...(typeof args.region === "string" ? { region: args.region } : {}),
          ...(args.fields && typeof args.fields === "object" ? { fields: args.fields } : {}),
        });
        break;
      }
      case "agent_keys.remove":
        this.keys = this.keys.filter((k) => (typeof args.slot === "string" ? !(k.env === args.env && k.slot === args.slot) : k.env !== args.env));
        break;
      case "agent_keys.use": {
        const app = String(args.app ?? "");
        if (typeof args.env === "string") this.uses[app] = { provider: "anthropic", env: args.env, ...(typeof args.slot === "string" ? { slot: args.slot } : {}) };
        else delete this.uses[app];
        break;
      }
      default:
        throw new ToolError(`unknown tool ${tool}`);
    }
    const report: Record<string, unknown> = { keys: this.keys, providers: [], available: this.available, ...(Object.keys(this.uses).length ? { uses: this.uses } : {}) };
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
      { id: "claude-code", name: "Claude Code", installed: true, skillsDir: "~/.claude/skills", mcpConfig: "~/.claude.json", cuaConfigured: false, skillsInstalled: [], skillsOutdated: [], error: undefined, signIn: undefined, signedIn: false },
      { id: "codex", name: "Codex", installed: true, skillsDir: "~/.agents/skills", mcpConfig: "~/.codex/config.toml", cuaConfigured: true, skillsInstalled: ["cua-spaces", "cua-sandbox"], skillsOutdated: [], error: undefined, signIn: "ChatGPT", signedIn: false },
      { id: "cursor", name: "Cursor", installed: false, skillsDir: undefined, mcpConfig: undefined, cuaConfigured: false, skillsInstalled: [], skillsOutdated: [], error: undefined, signIn: undefined, signedIn: false },
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

  async connectionStatusJson(_refresh: boolean) {
    const agents = this.current.map((a) => ({
      id: a.id,
      name: a.name,
      installed: a.installed,
      version: null,
      previous_version: null,
      signed_in: a.signIn === undefined ? "unknown" : a.signedIn ? "yes" : "no",
      sign_in: a.signIn === undefined ? null : `${a.signIn} sign-in`,
      mcp: a.mcpConfig === undefined ? "unsupported" : a.cuaConfigured ? "ok" : "missing",
      mcp_config: a.mcpConfig ?? null,
      signin_imported: false,
      signin_import: a.signIn === undefined ? "unsupported" : "missing",
      state: !a.installed ? "not_installed" : !a.cuaConfigured ? "not_connected" : a.signIn !== undefined && !a.signedIn ? "attention" : "connected",
      problem: a.installed && a.cuaConfigured && a.signIn !== undefined && !a.signedIn ? { line: `Connected. No ${a.signIn} sign-in on This Mac yet.`, action: "sign_in" } : null,
      checked_at_ms: 0,
    }));
    return JSON.stringify({ agents, checked_at_ms: 0 });
  }

  async connectJson(agents: string[], signIn: boolean, skills: boolean, mcp: boolean) {
    await this.setUp(agents, skills, mcp);
    this.calls.push(`connect:${agents.join(",")}:${signIn}`);
    return JSON.stringify(agents.map((agent) => ({ agent, tools_ok: true, tools_line: null, sign_in: null })));
  }

  async disconnectJson(agents: string[], _signIn: boolean) {
    await this.remove(agents);
    return JSON.stringify({ outcomes: [], removed_sign_ins: [] });
  }

  async disconnectPartsJson(agents: string[], skills: boolean, mcp: boolean, _signIn: boolean) {
    this.calls.push(`disconnect-parts:${agents.join(",")}:${skills}:${mcp}`);
    return JSON.stringify({ outcomes: [], removed_sign_ins: [] });
  }

  /** Codex's device code, approved at once (no browser in a fixture). */
  async deviceSignInStartJson(agent: string) {
    this.calls.push(`device-start:${agent}`);
    return JSON.stringify({ id: `${agent}-1`, url: "https://auth.openai.com/codex/device", code: "ABCD-12345" });
  }

  async deviceSignInWaitJson(id: string) {
    this.calls.push(`device-wait:${id}`);
    return JSON.stringify({ imported: { source: "codex", env: "CODEX_AUTH_JSON", label: "ChatGPT sign-in for Codex", expires_ms: null, added_ms: 1 }, refused: null, line: null });
  }

  /** The sign-in finishes at once (no browser in a fixture). */
  async signIn(agent: string) {
    this.calls.push(`sign-in:${agent}`);
    const a = this.current.find((s) => s.id === agent);
    if (!a?.signIn) throw new Error(`${a?.name ?? agent} has no sign-in cua can start`);
    a.signedIn = true;
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
  "agents.list", "agents.runs", "agents.events", "agents.start", "agents.message", "agents.pause", "agents.resume", "agents.setup", "agents.configure", "agents.signIn", "agents.setupDriver",
  "agents.detail", "agents.readFile", "agents.restoreFile", "agents.addRoutine", "agents.setRoutine", "agents.removeRoutine",
  "agents.allowComputer", "agents.revokeComputer",
  "agents.status", "agents.connect", "agents.disconnect", "agents.signInStart", "agents.signInWait",
  "agentKeys.list", "agentKeys.check", "agentKeys.set", "agentKeys.remove", "agentKeys.use", "agentKeys.shell", "agentKeys.fromShell",
];
