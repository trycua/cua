// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Bridge for the Settings "AI agents" + "Teleport permissions" sections:
 * the SDK's agent onboarding (cua skills and the cua MCP server in each
 * installed coding agent), and the per-app unattended-teleport policy.
 * No-ops off Tauri (browser dev / tests).
 */

import { hasTauri } from "./bridge";

/**
 * One AI coding agent as the SDK's agent onboarding (`cua-agent-setup`, the
 * same engine as `cua agents setup`) sees it.
 */
export interface AgentRow {
  agent: string;
  name: string;
  installed: boolean;
  /** cua skills and the cua MCP server are both in place (where supported). */
  configured: boolean;
  detail: string;
  skillsInstalled: number;
  skillsTotal: number;
  mcpConfig: string | null;
  skillsDir: string | null;
}

/** A teleportable app + its current per-app unattended-teleport allowances. */
export interface TeleportAppInfo {
  id: string;
  name: string;
  installed: boolean;
  allowSensitive: boolean;
  allowNonSensitive: boolean;
}

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke: tauriInvoke } = await import("@tauri-apps/api/core");
  return tauriInvoke<T>(command, args);
}

/** Every supported agent, installed ones first (read-only). */
export async function detectAgents(): Promise<AgentRow[]> {
  if (!hasTauri()) return [];
  return invoke<AgentRow[]>("agent_setup_detect");
}

/**
 * Install the cua skills and configure the cua MCP server for `agents`
 * (every installed agent when omitted). Resolves to the updated rows.
 */
export async function configureAgents(agents?: string[]): Promise<AgentRow[]> {
  if (!hasTauri()) return [];
  return invoke<AgentRow[]>("agent_setup_configure", { agents: agents ?? null });
}

/** Remove what cua added for `agents`. Resolves to the updated rows. */
export async function removeAgents(agents: string[]): Promise<AgentRow[]> {
  if (!hasTauri()) return [];
  return invoke<AgentRow[]>("agent_setup_remove", { agents });
}

/** List the rcdp-teleportable apps installed on this host + their policy. */
export async function listTeleportableApps(): Promise<TeleportAppInfo[]> {
  if (!hasTauri()) return [];
  const raw = await invoke<
    Array<{ id: string; name: string; installed: boolean; allow_sensitive: boolean; allow_non_sensitive: boolean }>
  >("list_teleportable_apps");
  return raw.map((a) => ({
    id: a.id,
    name: a.name,
    installed: a.installed,
    allowSensitive: a.allow_sensitive,
    allowNonSensitive: a.allow_non_sensitive,
  }));
}

/**
 * Persist an app's unattended-teleport allowances. Biometric-gated in the shell,
 * so this rejects if the device owner declines — callers should surface that and
 * leave the checkbox unchanged.
 */
export async function setTeleportPolicy(
  app: string,
  allowSensitive: boolean,
  allowNonSensitive: boolean,
): Promise<void> {
  if (!hasTauri()) return;
  await invoke("set_teleport_policy", { app, allowSensitive, allowNonSensitive });
}
