// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { hasTauri } from "./bridge";

/**
 * First-run installer steps (plan §8.14): put the bundled `cua` CLI on PATH
 * and run agent onboarding (cua skills + the cua MCP server in each detected
 * AI coding agent). The shell's `installer_*` commands back this; agent
 * onboarding runs the bundled `cua agents …` (the SDK's cua-agent-setup), so
 * the app, `cua auth login` and install.sh behave the same.
 */

export interface CliInstallPlan {
  /** Bundled CLI inside the app; null when this build has none. */
  source?: string | null;
  /** Exact file the CLI is written to. */
  target: string;
  binDir: string;
  method?: "symlink" | "copy" | null;
  installed: boolean;
  upToDate: boolean;
  installedVersion?: string | null;
  bundledVersion?: string | null;
  /** Another `cua` earlier on PATH that would shadow the install. */
  shadowedBy?: string | null;
  onPath: boolean;
  /** Shell profile (or "user PATH") a PATH change would edit. */
  pathProfile?: string | null;
  /** Line that would be added to `pathProfile`. */
  pathLine?: string | null;
}

export interface AgentInfo {
  id: string;
  name: string;
  installed: boolean;
  skillsDir?: string | null;
  mcpConfig?: string | null;
  cuaConfigured: boolean;
  skillsInstalled: string[];
  skillsOutdated?: string[];
  error?: string | null;
}

export interface SkillInfo {
  name: string;
  description: string;
  version: string;
}

export interface AgentDetectReport {
  agents: AgentInfo[];
  skills: SkillInfo[];
}

export interface AgentSetupRequest {
  agents: string[];
  skills: boolean;
  mcp: boolean;
  /** Also set up background computer-use: the cua-driver skill and MCP server. */
  driver?: boolean;
}

export interface AgentSetupOutcome {
  agents: string[];
  /** "skill" | "mcp" */
  target: string;
  item: string;
  path: string;
  /** created / updated / unchanged / removed / skipped / failed */
  change: string;
  detail: string;
  backup?: string | null;
}

export interface AgentSetupReport {
  outcomes: AgentSetupOutcome[];
}

export interface InstallerBridge {
  readonly isNative: boolean;
  cliPlan(): Promise<CliInstallPlan>;
  installCli(request: { modifyPath: boolean }): Promise<CliInstallPlan>;
  detectAgents(): Promise<AgentDetectReport>;
  setupAgents(request: AgentSetupRequest): Promise<AgentSetupReport>;
}

export function createTauriInstallerBridge(): InstallerBridge {
  const core = import("@tauri-apps/api/core");
  const invoke = async <T>(command: string, args?: Record<string, unknown>) =>
    (await core).invoke<T>(command, args);
  return {
    isNative: true,
    cliPlan: () => invoke<CliInstallPlan>("installer_cli_plan"),
    installCli: (request) => invoke<CliInstallPlan>("installer_install_cli", { request }),
    detectAgents: () => invoke<AgentDetectReport>("installer_detect_agents"),
    setupAgents: (request) => invoke<AgentSetupReport>("installer_setup_agents", { request }),
  };
}

export function createFallbackInstallerBridge(): InstallerBridge {
  const unavailable = () => Promise.reject(new Error("The installer needs the Cua Spaces app"));
  return {
    isNative: false,
    cliPlan: unavailable,
    installCli: unavailable,
    detectAgents: unavailable,
    setupAgents: unavailable,
  };
}

export function createInstallerBridge(): InstallerBridge {
  return hasTauri() ? createTauriInstallerBridge() : createFallbackInstallerBridge();
}

/** In-memory installer for tests and design work. */
export function createFakeInstallerBridge(
  options: {
    plan?: Partial<CliInstallPlan>;
    agents?: AgentInfo[];
    skills?: SkillInfo[];
    failInstall?: string;
    failDetect?: string;
    failSetup?: string;
  } = {},
): InstallerBridge & { calls: string[]; setupRequests: AgentSetupRequest[] } {
  const calls: string[] = [];
  const setupRequests: AgentSetupRequest[] = [];
  let plan: CliInstallPlan = {
    source: "/Applications/Cua Spaces.app/Contents/MacOS/cua",
    target: "/Users/ada/.local/bin/cua",
    binDir: "/Users/ada/.local/bin",
    method: "symlink",
    installed: false,
    upToDate: false,
    bundledVersion: "cua 0.3.0",
    onPath: false,
    pathProfile: "/Users/ada/.zshrc",
    pathLine: 'export PATH="/Users/ada/.local/bin:$PATH"',
    ...options.plan,
  };
  const agents: AgentInfo[] = options.agents ?? [
    {
      id: "claude-code",
      name: "Claude Code",
      installed: true,
      mcpConfig: "/Users/ada/.claude.json",
      skillsDir: "/Users/ada/.claude/skills",
      cuaConfigured: false,
      skillsInstalled: [],
    },
    {
      id: "codex",
      name: "Codex",
      installed: true,
      mcpConfig: "/Users/ada/.codex/config.toml",
      cuaConfigured: true,
      skillsInstalled: ["cua-driver"],
    },
    { id: "cursor", name: "Cursor", installed: false, cuaConfigured: false, skillsInstalled: [] },
  ];
  const skills: SkillInfo[] = options.skills ?? [
    { name: "cua-driver", description: "Drive this computer's apps", version: "0.3.0" },
    { name: "cua-sandbox", description: "Create and control sandboxes", version: "0.3.0" },
    { name: "cua-spaces", description: "Work in Cua Spaces", version: "0.3.0" },
  ];
  return {
    isNative: true,
    calls,
    setupRequests,
    cliPlan: async () => {
      calls.push("cliPlan");
      return plan;
    },
    installCli: async ({ modifyPath }) => {
      calls.push(`installCli:${modifyPath ? "path" : "nopath"}`);
      if (options.failInstall) throw new Error(options.failInstall);
      plan = {
        ...plan,
        installed: true,
        upToDate: true,
        installedVersion: plan.bundledVersion,
        onPath: plan.onPath || modifyPath,
        pathProfile: modifyPath ? null : plan.pathProfile,
        pathLine: modifyPath ? null : plan.pathLine,
      };
      return plan;
    },
    detectAgents: async () => {
      calls.push("detectAgents");
      if (options.failDetect) throw new Error(options.failDetect);
      return { agents, skills };
    },
    setupAgents: async (request) => {
      calls.push(
        `setupAgents:${request.agents.join(",")}:${request.skills ? "skills" : ""}:${request.mcp ? "mcp" : ""}${request.driver ? ":driver" : ""}`,
      );
      setupRequests.push(request);
      if (options.failSetup) throw new Error(options.failSetup);
      const outcomes: AgentSetupOutcome[] = [];
      for (const id of request.agents) {
        if (request.mcp)
          outcomes.push({ agents: [id], target: "mcp", item: "cua", path: `/cfg/${id}`, change: "created", detail: "" });
        if (request.skills)
          for (const s of skills)
            outcomes.push({ agents: [id], target: "skill", item: s.name, path: `/skills/${id}/${s.name}`, change: "created", detail: "" });
      }
      // `cua agents setup --cua-driver`: the cua-driver skill and MCP server.
      if (request.driver)
        for (const id of request.agents) {
          const had = request.skills && skills.some((s) => s.name === "cua-driver");
          outcomes.push({
            agents: [id],
            target: "skill",
            item: "cua-driver",
            path: `/skills/${id}/cua-driver`,
            change: had ? "unchanged" : "created",
            detail: "",
          });
          outcomes.push({ agents: [id], target: "mcp", item: "cua-driver", path: `/cfg/${id}`, change: "created", detail: "" });
        }
      return { outcomes };
    },
  };
}
