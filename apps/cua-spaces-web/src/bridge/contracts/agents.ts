// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Agents: persistent agents, their runs in Spaces, a run's events, and the
 * coding agents on this machine. Hand-written mirrors, each naming its
 * source:
 *
 * - `PersistentAgent`: `cua-spaces-app-core/src/persistent.rs`
 *   `PersistentAgentInput` (what `persistent_agent_list` reports, camelCase);
 * - `SpaceAgentRun`: `cua-spaces-app-core/src/agents.rs` `SpaceAgentRun`
 *   (the Tauri `list_space_agents` row);
 * - `AgentEvent`, `AgentEventsPage`: `cua-agents/src/events.rs` `AgentEvent`
 *   and the `agent_events` tool's result (snake_case, as the tool answers);
 * - `AgentSetupRow`: `cua-spaces-app-core/src/agents.rs` `AgentSettingsRow`
 *   (the Tauri `agent_setup_detect` row).
 */

/** cua-agents' `RunStatus`, verbatim. `unknown` is never a guess. */
export type AgentRunStatus = "running" | "idle" | "failed" | "crashed" | "unknown";

export interface PersistentAgent {
  name: string;
  /** Harness id (`claude-code`, `openai-codex`, `hermes`, `openclaw`, ...). */
  harness: string;
  /** The Space it works in. */
  space: string;
  paused: boolean;
  /** `running`, `suspended` or `released`. */
  spaceState: string;
  /** Its current run, if any. */
  runId?: string | null;
  /** Unix ms of the last home save (0: never). */
  savedMs: number;
  lastError?: string | null;
}

export interface SpaceAgentRun {
  runId: string;
  /** Harness id. */
  agent: string;
  status: AgentRunStatus;
  /** Why it is in that status. */
  reason: string;
  /** The run's label or prompt, on one line. */
  summary: string;
  /** Unix seconds. */
  createdAt: number | null;
  /** Finer phase (`thinking`, `tool`, `waiting`, ...). */
  phase: string;
  turn: number;
  /** The run is blocked until the person acts (a sign-in, an approval). A host that cannot tell leaves it out. */
  needsYou?: { title: string; detail: string } | null;
}

/** How a conversation view shows an event (`cua_agents::events::CATEGORIES`). */
export type AgentEventCategory = "message" | "user" | "activity" | "hidden";

/** One normalized event of a run. The same kinds for every harness. */
export interface AgentEvent {
  /** Monotonic within a run. */
  seq: number;
  ts_ms: number;
  /** 0 before the first prompt. */
  turn: number;
  /** `turn_started`, `message`, `thought`, `tool_call`, `tool_update`, `plan`, `turn_ended`, `error`, ... */
  kind: string;
  /** Message, thought or prompt text (a message arrives in chunks). */
  text?: string;
  tool_id?: string;
  tool_title?: string;
  /** ACP tool kind (`execute`, `edit`, `read`, ...). */
  tool_kind?: string;
  /** ACP tool status (`pending`, `in_progress`, `completed`, `failed`). */
  tool_status?: string;
  stop_reason?: string;
  category: AgentEventCategory;
  /** One line for an `activity` event. */
  summary?: string;
}

/** `agent_events`: the events after a cursor. */
export interface AgentEventsPage {
  run_id: string;
  status: AgentRunStatus;
  phase: string;
  events: AgentEvent[];
  /** Pass back to continue. */
  cursor: number;
  /** Nothing more is written yet. */
  caught_up: boolean;
}

/** `agents.start`: a coding agent's run in a Space (`agent_start`). */
export interface AgentStartRequest {
  spaceId: string;
  /** Harness id (`claude-code`, `openai-codex`, ...). */
  agent: string;
  /** The task. */
  prompt: string;
  /** A Git URL cloned into the run's folder first, and its branch or tag. */
  repo?: string;
  branch?: string;
  model?: string;
  /** Provider key names to forward (values never cross the bridge). Saved keys the harness reads go without being named. */
  envFromHost?: string[];
}

/** `agent_start`'s answer (snake_case, as the tool answers). */
export interface AgentStarted {
  run_id: string;
  agent: string;
  space?: string;
  cwd?: string;
  hint?: string;
}

/** `agent_message`'s answer: queued while a turn runs, or a new turn. */
export interface AgentMessageSent {
  delivered: boolean;
  run_id: string;
  status: AgentRunStatus;
  phase: string;
  reason?: string;
}

/** A coding agent on this machine, as the SDK's agent setup sees it. */
export interface AgentSetupRow {
  /** Setup id (`claude-code`, `codex`, `hermes`, `openclaw`, ...). */
  agent: string;
  name: string;
  installed: boolean;
  /** The cua skills and the cua MCP server are both in place (where supported). */
  configured: boolean;
  detail: string;
  skillsInstalled: number;
  skillsTotal: number;
  /** The MCP config file cua edits, when the agent has one. */
  mcpConfig: string | null;
  skillsDir: string | null;
  /** The account it signs in with on its own (`ChatGPT` for Codex), when the setup can see that sign-in. */
  signIn: string | null;
  /** That sign-in is on this computer. */
  signedIn: boolean;
}

/* ---- One persistent agent's detail ---------------------------------------------
 * `cua-spaces-app-core/src/persistent.rs`: what the page reads
 * (`AgentsInput`), its state and actions (`AgentsState`, `AgentsAction`,
 * `AgentsRequest`) and what it draws (`AgentDetailView`). The page runs
 * `agents.pageReduce` / `agents.pageView` in the core, as the SwiftUI
 * app's `PersistentModel` does; the host runs the tools each request names.
 */

/** A file in the agent's home (`volume_ls`, files only). */
export interface AgentHomeEntry {
  path: string;
  name: string;
  size: number;
}

/** One version of a file (`volume_history`, newest first). */
export interface AgentFileVersion {
  version: string;
  modifiedMs: number;
  deleted: boolean;
  latest: boolean;
}

/** An opened memory file (`volume_read` + `volume_history`). */
export interface AgentOpenFile {
  path: string;
  /** Empty for a binary file. */
  text: string;
  binary: boolean;
  versions: AgentFileVersion[];
}

/** A routine (`routine_list`). */
export interface AgentRoutine {
  id: string;
  title: string;
  /** `Every day at 8:00 AM`. */
  label: string;
  enabled: boolean;
}

/** A computer grant (`computer_access_list`). */
export interface AgentComputerGrant {
  agent: string;
  machine: string;
  revoked: boolean;
}

/** A computer-access audit line (`computer_access_list` with `audit`). */
export interface AgentAccessAudit {
  tsMs: number;
  /** `grant`, `revoke`, `use`, `denied`. */
  action: string;
  /** `agent:ada` or `user`. */
  principal: string;
  /** The machine. */
  path: string;
  detail: string;
}

/** `agents.detail`: the selected agent's home, routines, every grant and the recent audit. */
export interface AgentDetailInput {
  home: AgentHomeEntry[];
  routines: AgentRoutine[];
  grants: AgentComputerGrant[];
  audit: AgentAccessAudit[];
  /** This machine's Space id when it is set up for access (`relay:<id>`). */
  thisMachine: string | null;
}

/** `AgentsInput`: everything the core's Agents page reads. */
export interface AgentsPageInput extends Partial<Omit<AgentDetailInput, "thisMachine">> {
  agents: PersistentAgent[];
  file?: AgentOpenFile | null;
  thisMachine?: string | null;
}

export type AgentTab = "memory" | "routines" | "access";
export type RoutineScheduleKind = "every" | "daily" | "weekly";

/** The new-routine form (`RoutineForm`). */
export interface RoutineForm {
  title: string;
  prompt: string;
  schedule: RoutineScheduleKind;
  /** Minutes for `every`. */
  minutes: number;
  /** `HH:MM` for daily and weekly. */
  time: string;
  /** `mon` ... `sun` for weekly. */
  weekday: string;
}

/** The command the core asks the host to run (`AgentsRequest`). */
export type AgentsPageRequest =
  | { kind: "pause"; name: string }
  | { kind: "resume"; name: string }
  | { kind: "load"; name: string }
  | { kind: "read-file"; path: string }
  | { kind: "restore"; path: string; version: string }
  | {
      kind: "add-routine";
      agent: string;
      title: string;
      prompt: string;
      every_minutes: number | null;
      daily_at: string | null;
      weekly_on: string | null;
    }
  | { kind: "set-routine"; id: string; enabled: boolean }
  | { kind: "remove-routine"; id: string }
  | { kind: "allow"; agent: string; machine: string }
  | { kind: "revoke"; agent: string; machine: string };

/** The page's state (`AgentsState`). */
export interface AgentsPageState {
  selected?: string | null;
  tab: AgentTab;
  openPath?: string | null;
  form: RoutineForm;
  busy: boolean;
  error?: string | null;
  request?: AgentsPageRequest | null;
}

/** An input to the page (`AgentsAction`). */
export type AgentsPageAction =
  | { type: "select"; name: string }
  | { type: "set-tab"; tab: AgentTab }
  | { type: "open-file"; path: string }
  | { type: "close-file" }
  | { type: "pause"; name: string }
  | { type: "resume"; name: string }
  | { type: "restore"; version: string }
  | { type: "set-title"; title: string }
  | { type: "set-prompt"; prompt: string }
  | { type: "set-schedule"; schedule: RoutineScheduleKind }
  | { type: "set-minutes"; minutes: number }
  | { type: "set-time"; time: string }
  | { type: "set-weekday"; weekday: string }
  | { type: "add-routine" }
  | { type: "toggle-routine"; id: string }
  | { type: "remove-routine"; id: string }
  | { type: "allow-computer"; machine: string }
  | { type: "revoke-computer"; machine: string }
  | { type: "done" }
  | { type: "failed"; error: string };

/** One line with an id and an optional action (`LineView`). */
export interface AgentLineView {
  id: string;
  text: string;
  trailing: string;
  actionLabel?: string | null;
  secondaryLabel?: string | null;
  on?: boolean | null;
}

export interface AgentTabView {
  tab: AgentTab;
  label: string;
  selected: boolean;
}

/** The open memory file (`FileView`). */
export interface AgentFileView {
  path: string;
  text: string;
  versions: AgentLineView[];
  closeLabel: string;
}

/** The selected agent (`AgentDetailView`). */
export interface AgentDetailView {
  name: string;
  /** `Home agents/ada/, saved 5m ago`. */
  subtitle: string;
  tabs: AgentTabView[];
  memory: AgentLineView[];
  memoryEmpty: string;
  file?: AgentFileView | null;
  routines: AgentLineView[];
  routinesEmpty: string;
  form: RoutineForm;
  /** The schedule choices; `on` marks the form's. */
  schedules: AgentLineView[];
  canAddRoutine: boolean;
  addRoutineLabel: string;
  access: AgentLineView[];
  accessEmpty: string;
  /** `Allow on this computer` when this machine is set up and not granted. */
  allowThisMachineLabel?: string | null;
  audit: AgentLineView[];
  error?: string | null;
}

/** The page as drawn (`AgentsView`); the detail is what the agent detail shows. */
export interface AgentsPageView {
  title: string;
  detail?: AgentDetailView | null;
  busy: boolean;
  error?: string | null;
  requestText?: string | null;
}

/**
 * Whether each agent on This Mac is really connected (`agents.status`;
 * `cua_agent_setup::connection::ConnectionRow`, CONTRACT §4). snake_case, as
 * the status serializes it; the same JSON `cua connect status --json` prints
 * and the `connect_status` MCP tool answers.
 */
export interface ConnectionRow {
  id: string;
  name: string;
  installed: boolean;
  version: string | null;
  previous_version: string | null;
  signed_in: "yes" | "no" | "unknown" | "expired";
  sign_in: string | null;
  mcp: "ok" | "missing" | "broken" | "stale" | "unsupported";
  mcp_config: string | null;
  signin_imported: boolean;
  signin_import: "ok" | "expired" | "missing" | "unsupported";
  state: "connected" | "attention" | "not_connected" | "not_installed";
  problem: { line: string; action: "reconnect" | "sign_in" | "install" | "disconnect" | "none" } | null;
  checked_at_ms: number;
}

export interface ConnectionReport {
  agents: ConnectionRow[];
  checked_at_ms: number;
}

/** A row as drawn (`agents.connectionRows`, `cua-spaces-app-core/src/connection.rs`). */
export interface ConnectionRowView {
  id: string;
  name: string;
  state: "connected" | "attention" | "not_connected";
  label: string;
  problem: boolean;
  detail: string | null;
  signIn: string | null;
  /** `connect`, `reconnect`, `sign-in`, `disconnect`, `install`. */
  action: { id: string; label: string } | null;
}

/** What Connect did for one agent (`cua_agent_setup::connection::ConnectResult`). */
export interface ConnectResult {
  agent: string;
  tools_ok: boolean;
  tools_line: string | null;
  sign_in: {
    imported: { source: string; env: string; label: string; expires_ms: number | null; added_ms: number } | null;
    /** `not_signed_in`, `cancelled`, `not_yet`, `failed`. */
    refused: string | null;
    line: string | null;
  } | null;
}
