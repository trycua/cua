// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The coding apps a task can run with (`agent_start`'s harnesses), in the picker's order. */
export const TASK_AGENTS: { id: string; name: string; setupId: string }[] = [
  { id: "claude-code", name: "Claude Code", setupId: "claude-code" },
  { id: "openai-codex", name: "Codex", setupId: "codex" },
  { id: "gemini-cli", name: "Gemini CLI", setupId: "gemini-cli" },
  { id: "opencode", name: "OpenCode", setupId: "opencode" },
  { id: "goose", name: "Goose", setupId: "goose" },
  { id: "hermes", name: "Hermes", setupId: "hermes" },
];

export const agentName = (id: string) => TASK_AGENTS.find((a) => a.id === id)?.name ?? id;

/** The agents Home's picker offers: only the ones on this computer, in the picker's order. */
export const offeredAgents = (installed: ReadonlySet<string>) => TASK_AGENTS.filter((a) => installed.has(a.id));

/**
 * Who does a job: the person's pick, else the one they use most, else the
 * first one on this computer, each only while it is on this computer. Null
 * when none is: Home offers to install Claude Code instead of a name it
 * can't run.
 */
export function chooseAgent(picked: string | null, mostUsed: string | null, installed: ReadonlySet<string>): string | null {
  for (const id of [picked, mostUsed]) if (id && installed.has(id)) return id;
  return offeredAgents(installed)[0]?.id ?? null;
}

/** Where Claude Code is installed from, for "Install Claude Code". */
export const CLAUDE_CODE_URL = "https://claude.com/product/claude-code";
