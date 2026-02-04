/**
 * CuaBot Settings Management
 */

import { existsSync, readFileSync, writeFileSync, mkdirSync } from "fs";
import { join } from "path";
import { homedir } from "os";

const CONFIG_DIR = join(homedir(), ".cuabot");
const SETTINGS_FILE = join(CONFIG_DIR, "settings.json");

export interface Settings {
  defaultAgent?: string;
  telemetryEnabled?: boolean;
  aliasIgnored?: boolean;
}

export const AGENTS = {
  claude: {
    name: "Claude Code",
    description: "Anthropic's Claude AI coding assistant",
    command: "claude",
  },
  gemini: {
    name: "Gemini CLI",
    description: "Google's Gemini AI assistant",
    command: "npx @google/gemini-cli",
  },
  codex: {
    name: "OpenAI Codex",
    description: "OpenAI's Codex coding assistant",
    command: "codex",
  },
  aider: {
    name: "Aider",
    description: "AI pair programming in your terminal",
    command: "aider",
  },
  openclaw: {
    name: "OpenClaw",
    description: "OpenClaw AI assistant",
    command: "openclaw",
  },
  vibe: {
    name: "Vibe",
    description: "Mistral's Vibe coding assistant",
    command: "vibe",
  },
} as const;

export type AgentId = keyof typeof AGENTS;

function ensureConfigDir() {
  if (!existsSync(CONFIG_DIR)) {
    mkdirSync(CONFIG_DIR, { recursive: true });
  }
}

export function loadSettings(): Settings {
  ensureConfigDir();
  if (!existsSync(SETTINGS_FILE)) {
    return {};
  }
  try {
    return JSON.parse(readFileSync(SETTINGS_FILE, "utf-8"));
  } catch {
    return {};
  }
}

export function saveSettings(settings: Settings): void {
  ensureConfigDir();
  writeFileSync(SETTINGS_FILE, JSON.stringify(settings, null, 2));
}

export function getDefaultAgent(): string | undefined {
  return loadSettings().defaultAgent;
}

export function setDefaultAgent(agent: string): void {
  const settings = loadSettings();
  settings.defaultAgent = agent;
  saveSettings(settings);
}

export function getTelemetryEnabled(): boolean {
  const settings = loadSettings();
  const envValue = process.env.CUABOT_TELEMETRY?.toLowerCase() === "true";
  const envFalse = process.env.CUABOT_TELEMETRY?.toLowerCase() === "false";

  // If any source is explicitly false, return false
  if (settings.telemetryEnabled === false || envFalse) {
    return false;
  }
  // If any source is true, return true
  if (settings.telemetryEnabled === true || envValue) {
    return true;
  }
  // Default to false
  return false;
}

export function setTelemetryEnabled(enabled: boolean): void {
  const settings = loadSettings();
  settings.telemetryEnabled = enabled;
  saveSettings(settings);
}

export function getAliasIgnored(): boolean {
  return loadSettings().aliasIgnored === true;
}

export function setAliasIgnored(ignored: boolean): void {
  const settings = loadSettings();
  settings.aliasIgnored = ignored;
  saveSettings(settings);
}
