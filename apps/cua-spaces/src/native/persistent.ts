// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type {
  AccessAuditInput,
  AgentsRequest,
  ComputerGrantInput,
  DriveEntryInput,
  DriveGrantInput,
  DriveMountInput,
  DriveRequest,
  DriveRequestInput,
  DriveSyncInput,
  NotificationInput,
  OpenFileInput,
  PersistentAgentInput,
  RoutineInput,
} from '../model/persistent';
import { feedFromTool } from '../model/persistent';
import { hasTauri } from './bridge';

/** Runs one Spaces tool in the daemon (the shell's `agents_tool`). */
export type ToolCall = (tool: string, args?: Record<string, unknown>) => Promise<any>; // eslint-disable-line @typescript-eslint/no-explicit-any

/** Largest home listed (entries). */
const MAX_HOME_ENTRIES = 500;

/**
 * The Agents, Drive and Notifications pages' data and requests, over the
 * daemon's Spaces tools. Allowing an agent on a computer and approving a
 * drive request ask for presence in the daemon first.
 */
export interface AgentsBridge {
  agents(): Promise<PersistentAgentInput[]>;
  home(name: string): Promise<DriveEntryInput[]>;
  routines(name: string): Promise<RoutineInput[]>;
  access(): Promise<{ grants: ComputerGrantInput[]; audit: AccessAuditInput[] }>;
  readFile(path: string): Promise<OpenFileInput>;
  run(request: AgentsRequest): Promise<void>;
  /** The Volume page's requests and grants (no file listing: Finder is). */
  drive(): Promise<{
    requests: DriveRequestInput[];
    grants: DriveGrantInput[];
  }>;
  runDrive(request: DriveRequest): Promise<void>;
  /** The user's home folder, to show the mount point as `~/...`. */
  userHome(): Promise<string | null>;
  /** `volume_mount_status` as the tool answers it; null when it cannot. */
  mountStatus(): Promise<DriveMountInput | null>;
  /** `volume_sync_status` as the tool answers it; null when it cannot. */
  syncStatus(): Promise<DriveSyncInput | null>;
  notifications(): Promise<NotificationInput[]>;
  markAllRead(): Promise<void>;
}

/** A tool's JSON object answer, or null when the daemon cannot answer. */
function objectOrNull(answer: Promise<any>): Promise<any> {
  return answer.then(
    (r) => (r !== null && typeof r === 'object' && !Array.isArray(r) ? r : null),
    () => null,
  );
}

/* eslint-disable @typescript-eslint/no-explicit-any */
export function createAgentsBridgeOver(
  call: ToolCall,
  reveal: (path: string) => Promise<void> = () => Promise.reject(new Error('Showing files needs the Cua Spaces app')),
  userHome: () => Promise<string | null> = async () => null,
): AgentsBridge {
  const entry = (e: any): DriveEntryInput => ({
    path: e.path,
    name: e.name,
    folder: Boolean(e.folder),
    size: e.size ?? 0,
  });
  return {
    agents: async () =>
      ((await call('persistent_agent_list')).agents ?? []).map((a: any) => ({
        name: a.name,
        harness: a.harness,
        space: a.space,
        paused: Boolean(a.paused),
        spaceState: a.space_state ?? 'running',
        runId: a.run_id ?? null,
        savedMs: a.saved_ms ?? 0,
        lastError: a.last_error ?? null,
      })),
    home: async (name) => {
      const out: DriveEntryInput[] = [];
      const queue = [`agents/${name}/`];
      // Bounded: at most MAX_HOME_ENTRIES entries and 50 folders.
      for (let folders = 0; queue.length > 0 && folders < 50 && out.length < MAX_HOME_ENTRIES; folders++) {
        const path = queue.shift() as string;
        const r = await call('volume_ls', { path });
        for (const e of (r.entries ?? []).map(entry)) {
          if (e.folder) queue.push(e.path);
          else out.push(e);
        }
      }
      return out;
    },
    routines: async (name) =>
      ((await call('routine_list', { agent: name })).routines ?? []).map((r: any) => ({
        id: r.id,
        title: r.title,
        label: r.label ?? '',
        enabled: r.isEnabled !== false,
      })),
    access: async () => {
      const r = await call('computer_access_list', { audit: 20 });
      return {
        grants: (r.grants ?? []).map((g: any) => ({ agent: g.agent, machine: g.machine, revoked: Boolean(g.revoked) })),
        audit: (r.audit ?? []).map((e: any) => ({
          tsMs: e.ts_ms,
          action: e.action,
          principal: e.principal,
          path: e.path,
          detail: e.detail ?? '',
        })),
      };
    },
    readFile: async (path) => {
      const [f, h] = await Promise.all([call('volume_read', { path }), call('volume_history', { path })]);
      const binary = f.encoding === 'base64';
      return {
        path,
        text: binary ? '' : f.content ?? '',
        binary,
        versions: (h.versions ?? []).map((v: any) => ({
          version: v.version,
          modifiedMs: v.modified_ms ?? 0,
          deleted: Boolean(v.deleted),
          latest: Boolean(v.latest),
        })),
      };
    },
    run: async (r) => {
      switch (r.kind) {
        case 'pause':
          await call('agent_pause', { name: r.name });
          return;
        case 'resume':
          await call('agent_resume', { name: r.name });
          return;
        case 'load':
        case 'read-file':
          return; // the page loads these itself
        case 'restore':
          await call('volume_restore', { path: r.path, version: r.version });
          return;
        case 'add-routine':
          await call('routine_add', {
            agent: r.agent,
            title: r.title,
            prompt: r.prompt,
            every_minutes: r.every_minutes,
            daily_at: r.daily_at,
            weekly_on: r.weekly_on,
          });
          return;
        case 'set-routine':
          await call('routine_set_enabled', { id: r.id, enabled: r.enabled });
          return;
        case 'remove-routine':
          await call('routine_remove', { id: r.id });
          return;
        case 'allow':
          await call('computer_access_grant', { agent: r.agent, machine: r.machine });
          return;
        case 'revoke':
          await call('computer_access_revoke', { agent: r.agent, machine: r.machine });
          return;
      }
    },
    drive: async () => {
      const [req, gr] = await Promise.all([call('volume_requests'), call('volume_grants')]);
      return {
        requests: (req.requests ?? []).map((q: any) => ({
          id: q.id,
          principal: q.principal,
          prefix: q.prefix,
          mode: q.mode,
          reason: q.reason ?? '',
        })),
        grants: (gr.grants ?? []).map((g: any) => ({
          id: g.id,
          principal: g.principal,
          prefix: g.prefix,
          mode: g.mode,
          revoked: Boolean(g.revoked),
        })),
      };
    },
    runDrive: async (r) => {
      switch (r.kind) {
        case 'load':
          return; // the page loads itself
        case 'mount-and-reveal': {
          // Mount, then show the mount point; a mount that did not finish
          // says why.
          const status = await call('volume_mount');
          if (status?.state === 'mounted' && status.path) {
            await reveal(status.path);
            return;
          }
          throw new Error(status?.detail || 'The volume could not be mounted');
        }
        case 'approve':
          await call('volume_approve', { request_id: r.id });
          return;
        case 'deny':
          await call('volume_deny', { request_id: r.id });
          return;
        case 'revoke':
          await call('volume_revoke', { grant_id: r.id });
          return;
        case 'reveal':
          await reveal(r.path);
          return;
        case 'resolve':
          await call('volume_sync_resolve', { path: r.path });
          return;
      }
    },
    userHome: () => userHome().catch(() => null),
    mountStatus: () => objectOrNull(call('volume_mount_status')),
    syncStatus: () => objectOrNull(call('volume_sync_status')),
    notifications: async () => feedFromTool((await call('notifications_list')).notifications ?? []),
    markAllRead: async () => {
      await call('notifications_ack', { ids: [] });
    },
  };
}
/* eslint-enable @typescript-eslint/no-explicit-any */

/** The shell's `agents_tool` (a rejecting stand-in outside it). */
export function agentsToolCall(): ToolCall {
  if (hasTauri()) {
    const core = import('@tauri-apps/api/core');
    return async (tool, args) => (await core).invoke('agents_tool', { tool, args: args ?? {} });
  }
  return () => Promise.reject(new Error('Agents need the Cua Spaces app'));
}

/** The shell's `drive_reveal` (a rejecting stand-in outside it). */
export function driveRevealCall(): (path: string) => Promise<void> {
  if (hasTauri()) {
    const core = import('@tauri-apps/api/core');
    return async (path) => (await core).invoke<void>('drive_reveal', { path });
  }
  return () => Promise.reject(new Error('Showing files needs the Cua Spaces app'));
}

/** The user's home folder from the shell (null outside it). */
function homeDirCall(): () => Promise<string | null> {
  if (hasTauri()) {
    return () => import('@tauri-apps/api/path').then(({ homeDir }) => homeDir());
  }
  return async () => null;
}

export function createAgentsBridge(): AgentsBridge {
  return createAgentsBridgeOver(agentsToolCall(), driveRevealCall(), homeDirCall());
}
