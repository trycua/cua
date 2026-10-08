// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Agents (the SwiftUI app's PersistentModel.swift agents half,
// AgentRunsModel.swift, AgentKeysModel.swift and the coding-agent setup of
// AppModel.swift and AppServices.swift): the persistent agents and a run's
// events over the daemon's tools, a Space's runs, the provider keys the
// daemon keeps, and the coding agents on this machine. Every word and rule
// is the app core's; this runs the tools and keeps the answers.
import type { Native } from "../native/load";
import type {
  AgentMcpServer,
  AgentSetupLike,
  AgentSetupOutcome,
  AppAgentKeyConfirm,
  AppAgentKeyFormView,
  AppAgentKeysInput,
  AppAgentKeysView,
  AppAgentSettingsRow,
  AppAgentSetupOutcomeInput,
  AppAgentSetupStatus,
  AppAgentsAction,
  AppAgentsState,
  AppAgentsView,
  AppSpaceAgentRun,
} from "../native/generated/index";
import { ToolError } from "./backend";
import { words } from "./errors";
import { NotStarted, type LiveGate, type LiveServices } from "./pending";

/** One of the daemon's Spaces tools (`agent_*`, `routine_*`, `agent_keys.*`), its JSON answer. */
export type ToolRunner = (tool: string, args: Record<string, unknown>) => Promise<unknown>;

type Json = Record<string, unknown>;
const arr = (o: Json, key: string): Json[] => (Array.isArray(o[key]) ? (o[key] as unknown[]).filter((v): v is Json => !!v && typeof v === "object") : []);

// MARK: Persistent agents

/** What the Agents page lists: the daemon's `persistent_agent_list` as `PersistentAgentInput` records. */
export interface PersistentAgentRecord {
  name: unknown;
  harness: unknown;
  space: unknown;
  paused: unknown;
  spaceState: unknown;
  runId: unknown;
  savedMs: unknown;
  lastError: unknown;
}

/** The persistent agents (the SwiftUI app's `PersistentModel`, agents half). */
export class PersistentAgents {
  /** The agents as last listed, with the selected one's detail (the core's input). */
  input: Json = { agents: [] };
  state: AppAgentsState;
  /** This machine's Space id when it is set up for access (`relay:<id>`). */
  thisMachine: () => string | null = () => null;
  onChange: () => void = () => {};

  constructor(
    private readonly native: Native,
    private readonly tools: ToolRunner | null,
  ) {
    this.state = native.appAgentsInitial();
  }

  /** The daemon's tools answer here (no daemon: the agents can't be read). */
  get canCallTools(): boolean {
    return this.tools !== null;
  }

  /** Persistent agents on this machine, once listed. */
  get count(): number {
    return (this.input.agents as unknown[]).length;
  }

  private async call(tool: string, args: Record<string, unknown> = {}): Promise<Json> {
    if (!this.tools) throw new ToolError("Agents need the cua daemon");
    const r = await this.tools(tool, args);
    return r && typeof r === "object" && !Array.isArray(r) ? (r as Json) : {};
  }

  typed() {
    return this.native.appAgentsInputFromJson(JSON.stringify({ ...this.input, thisMachine: this.thisMachine() }));
  }

  view(nowMs: bigint = BigInt(Date.now())): AppAgentsView {
    return this.native.appAgentsView(this.typed(), this.state, nowMs);
  }

  async load(): Promise<void> {
    try {
      await this.list();
    } catch {
      // A failed read leaves the list as it was.
    }
  }

  /** Reads the agents again and answers them (`PersistentAgentInput` records); a failed read throws and leaves the list as it was. */
  async list(): Promise<PersistentAgentRecord[]> {
    const r = await this.call("persistent_agent_list");
    const agents = arr(r, "agents").map((a) => ({
      name: a.name ?? "",
      harness: a.harness ?? "",
      space: a.space ?? "",
      paused: a.paused ?? false,
      spaceState: a.space_state ?? "running",
      runId: a.run_id ?? null,
      savedMs: a.saved_ms ?? 0,
      lastError: a.last_error ?? null,
    }));
    const before = JSON.stringify(this.input.agents);
    this.input.agents = agents;
    if (JSON.stringify(agents) !== before) this.onChange();
    return agents;
  }

  /** One run's events after `cursor`: the `agent_events` tool's answer as is. */
  async runEvents(space: string, runId: string, cursor: number, max: number | null): Promise<Json> {
    const args: Record<string, unknown> = { space, run_id: runId, cursor };
    if (max !== null) args.max = max;
    return this.call("agent_events", args);
  }

  private async loadDetail(name: string): Promise<void> {
    const files: Json[] = [];
    const queue = [`agents/${name}/`];
    let folders = 0;
    while (queue.length && folders < 50 && files.length < 500) {
      folders += 1;
      const path = queue.shift()!;
      for (const e of arr(await this.call("volume_ls", { path }), "entries")) {
        if (e.folder === true) queue.push(typeof e.path === "string" ? e.path : "");
        else files.push({ path: e.path ?? "", name: e.name ?? "", size: e.size ?? 0 });
      }
    }
    const routines = arr(await this.call("routine_list", { agent: name }), "routines").map((r) => ({
      id: r.id ?? "",
      title: r.title ?? "",
      label: r.label ?? "",
      enabled: typeof r.isEnabled === "boolean" ? r.isEnabled : true,
    }));
    const access = await this.call("computer_access_list", { audit: 20 });
    this.input.home = files;
    this.input.routines = routines;
    this.input.grants = arr(access, "grants").map((g) => ({ agent: g.agent ?? "", machine: g.machine ?? "", revoked: g.revoked ?? false }));
    this.input.audit = arr(access, "audit").map((a) => ({ tsMs: a.ts_ms ?? 0, action: a.action ?? "", principal: a.principal ?? "", path: a.path ?? "", detail: a.detail ?? "" }));
  }

  /** Reduces `action` through the core and runs the request it asks for (the page's Pause and Resume, the Agents page's actions). */
  async send(action: AppAgentsAction): Promise<void> {
    const before = this.state;
    this.state = this.native.appAgentsReduce(this.typed(), before, action);
    this.onChange();
    const request = this.state.request;
    if (before.busy || !request) return;
    try {
      switch (request.tag) {
        case "Load":
          await this.loadDetail(request.inner.name);
          break;
        case "ReadFile": {
          const path = request.inner.path;
          const [file, history] = await Promise.all([this.call("volume_read", { path }), this.call("volume_history", { path })]);
          const binary = file.encoding === "base64";
          this.input.file = {
            path,
            text: binary ? "" : (file.content ?? ""),
            binary,
            versions: arr(history, "versions").map((v) => ({ version: v.version ?? "", modifiedMs: v.modified_ms ?? 0, deleted: v.deleted ?? false, latest: v.latest ?? false })),
          };
          break;
        }
        case "Pause":
          await this.call("agent_pause", { name: request.inner.name });
          await this.load();
          break;
        case "Resume":
          await this.call("agent_resume", { name: request.inner.name });
          await this.load();
          break;
        case "Restore":
          await this.call("volume_restore", { path: request.inner.path, version: request.inner.version });
          break;
        case "AddRoutine": {
          const r = request.inner;
          const args: Record<string, unknown> = { agent: r.agent, title: r.title, prompt: r.prompt };
          if (r.everyMinutes !== undefined) args.every_minutes = r.everyMinutes;
          if (r.dailyAt !== undefined) args.daily_at = r.dailyAt;
          if (r.weeklyOn !== undefined) args.weekly_on = r.weeklyOn;
          await this.call("routine_add", args);
          break;
        }
        case "SetRoutine":
          await this.call("routine_set_enabled", { id: request.inner.id, enabled: request.inner.enabled });
          break;
        case "RemoveRoutine":
          await this.call("routine_remove", { id: request.inner.id });
          break;
        case "Allow":
          await this.call("computer_access_grant", { agent: request.inner.agent, machine: request.inner.machine });
          break;
        case "Revoke":
          await this.call("computer_access_revoke", { agent: request.inner.agent, machine: request.inner.machine });
          break;
      }
      // What changed something is read again for the open agent.
      if (!["Load", "ReadFile", "Pause", "Resume"].includes(request.tag) && this.state.selected !== undefined) {
        try {
          await this.loadDetail(this.state.selected);
        } catch {
          // The change went through; the detail is read again with the next visit.
        }
      }
      this.state = this.native.appAgentsReduce(this.typed(), this.state, new this.native.AppAgentsAction.Done());
    } catch (error) {
      this.state = this.native.appAgentsReduce(this.typed(), this.state, new this.native.AppAgentsAction.Failed({ error: words(error) }));
    }
    this.onChange();
  }
}

// MARK: A Space's runs

/** A Space's coding-agent runs for its detail. A failed read is its own state, never an empty list: "no agents" and "could not ask" are different claims. */
export type RunsLoad = { kind: "loading" } | { kind: "ready"; rows: AppSpaceAgentRun[] } | { kind: "failed"; message: string };

export class AgentRunsModel {
  load: RunsLoad = { kind: "loading" };

  constructor(
    private readonly native: Native,
    private readonly runs: (spaceId: string) => Promise<AppSpaceAgentRun[]>,
    readonly spaceId: string,
  ) {}

  /** Re-reads the runs. */
  async refresh(): Promise<void> {
    try {
      this.load = { kind: "ready", rows: await this.runs(this.spaceId) };
    } catch (error) {
      this.load = { kind: "failed", message: words(error) };
    }
  }

  /** One line per run: the agent, then what it was asked. */
  line(run: AppSpaceAgentRun): string {
    return `${this.native.appAgentName(run.agent)} · ${this.native.appAgentSubtitle(run)}`;
  }

  /** The status word. */
  status(run: AppSpaceAgentRun): string {
    return this.native.appAgentStatusLabel(run.status);
  }
}

// MARK: Provider keys

/** Settings → Agents: the provider keys agents get. The cua daemon keeps them in the system's credential store (`agent_keys.list/set/remove`, its app methods; no MCP tool reaches them) and answers only the provider, the variable, the last four characters and when each was added. A key passes through here once, in `save`, and is never kept. */
export class AgentKeysModel {
  /** The daemon's last answer, as it came. */
  report: Json | null = null;
  /** Why the keys could not be read. */
  error: string | null = null;

  constructor(
    private readonly native: Native,
    private readonly tools: ToolRunner | null,
  ) {}

  /** The daemon's methods answer here. */
  get available(): boolean {
    return this.tools !== null;
  }

  /** Runs one of the daemon's `agent_keys.*` methods; its answer (the list after it) replaces the shown one, and is returned as is. */
  async call(method: string, args: Record<string, unknown> = {}): Promise<unknown> {
    if (!this.tools) throw new ToolError("Agent keys need the cua daemon");
    const r = await this.tools(method, args);
    if (r && typeof r === "object" && !Array.isArray(r)) {
      this.report = r as Json;
      this.error = null;
    }
    return r;
  }

  async load(): Promise<void> {
    try {
      await this.call("agent_keys.list");
    } catch (error) {
      this.error = words(error);
    }
  }

  /** Adds or replaces a key (`env` names an Other key). */
  async save(provider: string, env: string | null, value: string): Promise<void> {
    const args: Record<string, unknown> = { provider, value };
    if (provider === "other" && env) args.env = env;
    await this.call("agent_keys.set", args);
  }

  async remove(env: string): Promise<void> {
    await this.call("agent_keys.remove", { env });
  }

  /** The core's input for the daemon's answer. */
  get input(): AppAgentKeysInput {
    return agentKeysInput(this.report, this.error ?? (this.available ? null : "Agent keys need the cua daemon"));
  }

  get view(): AppAgentKeysView {
    return this.native.appAgentKeysView(this.input);
  }

  form(provider: string, env: string | null, name: string, hasValue: boolean): AppAgentKeyFormView {
    return this.native.appAgentKeyForm(this.input, { provider, env: env ?? undefined, name, hasValue });
  }

  removeConfirm(env: string): AppAgentKeyConfirm | undefined {
    return this.native.appAgentKeyRemoveConfirm(this.input, env);
  }
}

/** The core's input for `agent_keys.list`'s answer (never a value). */
export function agentKeysInput(report: Json | null, error: string | null): AppAgentKeysInput {
  const keys = arr(report ?? {}, "keys")
    .map((k) => ({
      provider: typeof k.provider === "string" ? k.provider : "other",
      env: typeof k.env === "string" ? k.env : "",
      last4: typeof k.last4 === "string" ? k.last4 : "",
      addedMs: BigInt(typeof k.added_ms === "number" && k.added_ms >= 0 ? Math.floor(k.added_ms) : 0),
    }))
    .filter((k) => k.env !== "");
  const unavailable = report?.available === false ? (typeof report.unavailable === "string" ? report.unavailable : "this machine can't keep keys") : undefined;
  return { keys, unavailable, error: error ?? undefined };
}

// MARK: Coding agents on this machine

/** Coding agents on this machine (the SDK's agent onboarding). */
export interface AgentSetupRunning {
  /** Every supported agent, as the core reads it. */
  statuses(): Promise<AppAgentSetupStatus[]>;
  /** How many skills the SDK bundles. */
  skillsTotal(): number;
  /** Installs the skills and/or the MCP server for `agents`. */
  setUp(agents: string[], skills: boolean, mcp: boolean): Promise<AppAgentSetupOutcomeInput[]>;
  /** Background computer use for `agents`: the cua-driver skill and its MCP server (`cua agents setup --cua-driver`). */
  setUpCuaDriver(agents: string[]): Promise<AppAgentSetupOutcomeInput[]>;
  /** Removes what cua added for `agents`. */
  remove(agents: string[]): Promise<AppAgentSetupOutcomeInput[]>;
}

/** The SDK's `AgentSetup`, registering the app's own `cua` as the MCP server. */
export class LiveAgentSetup implements AgentSetupRunning {
  private readonly server: AgentMcpServer | undefined;

  constructor(
    private readonly native: Native,
    private readonly inner: AgentSetupLike,
    cuaBinary: string | null,
  ) {
    this.server = cuaBinary ? { name: "cua", command: cuaBinary, args: ["mcp"], env: new Map() } : undefined;
  }

  private outcomes(list: AgentSetupOutcome[]): AppAgentSetupOutcomeInput[] {
    return list.map((o) => this.native.appAgentSetupOutcome(o));
  }

  async statuses(): Promise<AppAgentSetupStatus[]> {
    return this.inner.detect().map((info) => this.native.appAgentSetupStatus(info));
  }

  skillsTotal(): number {
    return this.inner.skills().length;
  }

  async setUp(agents: string[], skills: boolean, mcp: boolean): Promise<AppAgentSetupOutcomeInput[]> {
    const out: AgentSetupOutcome[] = [];
    if (skills) out.push(...(await this.inner.installSkills(agents, [], false)));
    if (mcp) out.push(...(await this.inner.configureMcp(agents, this.server)));
    return this.outcomes(out);
  }

  async setUpCuaDriver(agents: string[]): Promise<AppAgentSetupOutcomeInput[]> {
    return this.outcomes(await this.inner.setupCuaDriver(agents, undefined, true, true));
  }

  async remove(agents: string[]): Promise<AppAgentSetupOutcomeInput[]> {
    return this.outcomes(await this.inner.remove(agents, true, true));
  }
}

/** The coding agents' setup until the SDK is in. */
export class PendingAgentSetup implements AgentSetupRunning {
  constructor(private readonly gate: LiveGate<LiveServices>) {}

  private async s(): Promise<AgentSetupRunning> {
    const s = (await this.gate.wait()).agentSetup;
    if (!s) throw new NotStarted("Agent setup");
    return s;
  }

  async statuses(): Promise<AppAgentSetupStatus[]> {
    return (await this.gate.wait()).agentSetup?.statuses() ?? [];
  }

  skillsTotal(): number {
    return this.gate.current?.agentSetup?.skillsTotal() ?? 0;
  }

  setUp = async (agents: string[], skills: boolean, mcp: boolean) => (await this.s()).setUp(agents, skills, mcp);
  setUpCuaDriver = async (agents: string[]) => (await this.s()).setUpCuaDriver(agents);
  remove = async (agents: string[]) => (await this.s()).remove(agents);
}

/** The Settings rows for the coding agents, and the changes made to them (the SwiftUI app's `AppModel.agentRows`, `configureAllAgents`, `agentAction`). */
export class CodingAgents {
  /** Null until read. */
  rows: AppAgentSettingsRow[] | null = null;
  busy = false;
  /** The agents with a change running. */
  pending: string[] = [];
  /** Why the last change failed, per agent. */
  private readonly failures = new Map<string, string>();
  onChange: () => void = () => {};

  constructor(
    private readonly native: Native,
    readonly setup: AgentSetupRunning | null,
  ) {}

  async reload(): Promise<void> {
    if (!this.setup) {
      this.rows = [];
      this.onChange();
      return;
    }
    const rows = this.native.appAgentSettingsRows(await this.setup.statuses(), this.setup.skillsTotal());
    this.rows = rows.map((r) => {
      const why = this.failures.get(r.agent);
      return why === undefined ? r : { ...r, configured: false, detail: why };
    });
    this.onChange();
  }

  /** "Configure all detected agents". */
  async configureAll(): Promise<void> {
    if (this.busy) return;
    const ids = (this.rows ?? []).filter((r) => r.installed).map((r) => r.agent);
    this.busy = true;
    this.onChange();
    try {
      await this.act(ids, false);
    } finally {
      this.busy = false;
      this.onChange();
    }
  }

  /** Sets up (or removes) the cua skills and MCP server for `ids`; a failure stays on its row. */
  async act(ids: string[], remove: boolean): Promise<void> {
    const setup = this.setup;
    if (!setup || ids.length === 0) return;
    this.pending = [...this.pending, ...ids];
    this.onChange();
    try {
      try {
        const outcomes = remove ? await setup.remove(ids) : await setup.setUp(ids, true, true);
        for (const id of ids) {
          const summary = this.native.appAgentSetupSummary(outcomes, id, id);
          if (summary.failed.length === 0) this.failures.delete(id);
          else this.failures.set(id, summary.failed.join("; "));
        }
      } catch (error) {
        for (const id of ids) this.failures.set(id, words(error));
      }
      await this.reload();
    } finally {
      for (const id of ids) {
        const at = this.pending.indexOf(id);
        if (at >= 0) this.pending.splice(at, 1);
      }
      this.onChange();
    }
  }

  /** A Settings row was pressed (`agent:<id>`): configure it, or remove what cua added when it is configured. */
  async press(row: string): Promise<void> {
    if (!row.startsWith("agent:")) return;
    const r = (this.rows ?? []).find((x) => `agent:${x.agent}` === row);
    if (r) await this.act([r.agent], r.configured);
  }
}

/** Everything Agents: the persistent agents, a Space's runs, the keys and the coding agents. */
export class AgentsModel {
  readonly persistent: PersistentAgents;
  readonly keys: AgentKeysModel;
  readonly coding: CodingAgents;

  constructor(
    readonly native: Native,
    tools: ToolRunner | null,
    setup: AgentSetupRunning | null,
    private readonly spaceRuns: (spaceId: string) => Promise<AppSpaceAgentRun[]>,
  ) {
    this.persistent = new PersistentAgents(native, tools);
    this.keys = new AgentKeysModel(native, tools);
    this.coding = new CodingAgents(native, setup);
  }

  /** What the page watches (`agents.changed`): the agents, the coding agents' rows and the changes running. */
  onChange(listener: () => void): void {
    this.persistent.onChange = listener;
    this.coding.onChange = listener;
  }

  runs(spaceId: string): AgentRunsModel {
    return new AgentRunsModel(this.native, this.spaceRuns, spaceId);
  }
}
