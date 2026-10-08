// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Agents on the real app core (`pnpm native`), over an in-memory daemon and
// coding agents (the SwiftUI app's AgentRunsTests, AgentKeysTests,
// AgentsPagesTests and the agents cases of WebUIHostTests, BridgeContractTests
// and UpdatesTests): the bridge answers the SwiftUI host's shapes, the keys
// never keep a value,. Skipped
// when this machine's native directory was not built.
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { validate, type Schema } from "../../cua-spaces-web/src/bridge/contracts/schema";
import { createBridge, type Bridge } from "../src/bridge";
import type { BridgeEvent } from "../src/bridge/host";
import { LiveAgentSetup, agentKeysInput, type ToolRunner } from "../src/model/agents";
import { AppModel } from "../src/model/app-model";
import { CloudModel } from "../src/model/cloud";
import { DevicesModel } from "../src/model/devices";
import { HostModel } from "../src/model/host";
import { StartupModel } from "../src/model/startup";
import { devDirName, nativeFiles } from "../src/native/location";
import { loadNative, type Native } from "../src/native/load";
import type { AgentInfo, AgentSetupLike, AgentSetupOutcome } from "../src/native/generated/index";
import { FakeAgentKeysTools, FakeAgentsTools, FixtureAgentSetup, RUNS } from "./agents-fixtures";
import { FixtureAccount, FixtureSpacesBackend, FixtureTelemetry, fixtureDevices, fixtureHost } from "./fixtures";

const dir = path.resolve(__dirname, "../native", devDirName(process.platform, process.arch));
const built = existsSync(nativeFiles(dir, process.platform).library);

const SHAPES = JSON.parse(readFileSync(path.resolve(__dirname, "../../cua-spaces-web/src/bridge/contracts/bridge-shapes.json"), "utf8")) as {
  webkit: Record<string, Schema>;
  $defs: Record<string, Schema>;
};
const shapeProblems = (method: string, answer: unknown) => validate(SHAPES.webkit[method]!, JSON.parse(JSON.stringify({ v: answer })).v as unknown, SHAPES.$defs).map((e) => `${method} ${e}`);
const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));

describe.skipIf(!built)("agents", () => {
  let home: string;
  let native: Native;
  const saved = { ...process.env };
  let backend: FixtureSpacesBackend;
  let telemetry: FixtureTelemetry;
  let daemon: FakeAgentsTools;
  let keys: FakeAgentKeysTools;
  let setup: FixtureAgentSetup;
  let events: BridgeEvent[];

  beforeAll(async () => {
    home = mkdtempSync(path.join(tmpdir(), "cua-agents-"));
    Object.assign(process.env, { HOME: home, USERPROFILE: home, CUA_HOME: path.join(home, ".cua"), CUA_TELEMETRY: "0", DO_NOT_TRACK: "1", CUA_KEYCHAIN_NONINTERACTIVE: "1" });
    native = await loadNative(dir);
  });

  afterAll(() => {
    process.env = saved;
    rmSync(home, { recursive: true, force: true });
  });

  beforeEach(() => {
    backend = new FixtureSpacesBackend(native);
    backend.fixtureRuns["local:aurora"] = RUNS;
    telemetry = new FixtureTelemetry();
    daemon = new FakeAgentsTools();
    keys = new FakeAgentKeysTools();
    setup = new FixtureAgentSetup();
    events = [];
  });

  /** A model on the fixtures, with the daemon's tools (by name) and coding agents given. */
  function makeBridge(o: { tools?: ToolRunner | null; setup?: FixtureAgentSetup | null } = {}): { model: AppModel; bridge: Bridge } {
    const tools: ToolRunner | null =
      o.tools === undefined ? (tool, args) => (tool.startsWith("agent_keys.") ? keys.run(tool, args) : daemon.run(tool, args)) : o.tools;
    const model = new AppModel({
      native,
      backend,
      startup: new StartupModel({ kind: "ready" }),
      settingsPath: path.join(mkdtempSync(path.join(home, "app-")), "app-settings.json"),
      host: new HostModel(native, fixtureHost()),
      devices: new DevicesModel(native, () => fixtureDevices()),
      cloud: new CloudModel(native, () => backend),
      account: new FixtureAccount("ada@example.com"),
      telemetry,
      agentSetup: o.setup === undefined ? setup : o.setup,
      agentTools: tools,
      servicesIn: () => true,
      cpus: 8,
    });
    const bridge = createBridge({ model, supervisor: null, version: "1.2.3", platform: "darwin", env: {}, ui: { openSpace: () => {}, setBackground: () => {}, activate: () => {} } });
    bridge.events.subscribe((e) => events.push(e));
    return { model, bridge };
  }

  const call = (b: Bridge, method: string, args: Record<string, unknown> = {}) => b.registry.handle(method, args);
  const code = (b: Bridge, method: string, args: Record<string, unknown> = {}) => b.registry.dispatch({ id: "x", method, args }).then((r) => (r.ok ? null : r.error.code));
  const message = (b: Bridge, method: string, args: Record<string, unknown> = {}) => b.registry.dispatch({ id: "x", method, args }).then((r) => (r.ok ? null : r.error.message));

  // MARK: A Space's runs (AgentRunsTests)

  it("lists a Space's runs as the core's rows, attention first", async () => {
    const { model } = makeBridge();
    const runs = model.agents.runs("local:aurora");
    expect(runs.load.kind).toBe("loading");
    await runs.refresh();
    if (runs.load.kind !== "ready") throw new Error(`not ready: ${runs.load.kind}`);
    const rows = runs.load.rows;
    expect(rows.map((r) => r.runId)).toEqual(["r-fail", "r-run", "r-bad"]);
    expect(runs.line(rows[0]!)).toBe("OpenAI Codex · triage");
    expect(runs.line(rows[1]!)).toBe("Claude Code · fix the build");
    expect(runs.status(rows[1]!)).toBe("Running");
    // A record that could not be read is still a row, status Unknown.
    expect(runs.line(rows[2]!)).toBe("Unknown agent · this run's record could not be read");
    expect(runs.status(rows[2]!)).toBe("Unknown");
  });

  it("keeps a failed read apart from an empty list", async () => {
    const { model } = makeBridge();
    backend.agentRunsError = new Error("timed out after 5 s");
    const runs = model.agents.runs("local:empty");
    await runs.refresh();
    expect(runs.load).toEqual({ kind: "failed", message: "timed out after 5 s" });
    backend.agentRunsError = null;
    await runs.refresh();
    expect(runs.load).toEqual({ kind: "ready", rows: [] });
  });

  it("answers agents.runs in the page's rows, or why it could not", async () => {
    const { bridge } = makeBridge();
    const runs = (await call(bridge, "agents.runs", { spaceId: "local:aurora" })) as { runId: string; status: string }[];
    expect(runs.map((r) => r.runId)).toEqual(["r-fail", "r-run", "r-bad"]);
    expect(runs[1]!.status).toBe("running");
    expect(shapeProblems("agents.runs", runs)).toEqual([]);
    backend.agentRunsError = new Error("space is gone");
    expect(await code(bridge, "agents.runs", { spaceId: "local:aurora" })).toBe("failed");
    expect(await message(bridge, "agents.runs", { spaceId: "local:aurora" })).toBe("space is gone");
  });

  // MARK: Persistent agents (AgentsPagesTests, WebUIHostTests)

  it("lists the persistent agents as camelCase records", async () => {
    const { bridge, model } = makeBridge();
    const agents = (await call(bridge, "agents.list")) as { name: string; spaceState: string; runId: string | null }[];
    expect(agents.map((a) => a.name)).toEqual(["ada", "scout"]);
    expect(agents[0]).toMatchObject({ runId: "run-1", lastError: null, spaceState: "running" });
    expect(agents[1]!.spaceState).toBe("released");
    expect(model.agents.persistent.count).toBe(2);
    expect(shapeProblems("agents.list", agents)).toEqual([]);
  });

  it("flows pause, select and allow through the core", async () => {
    const { model } = makeBridge();
    const p = model.agents.persistent;
    const A = native.AppAgentsAction;
    p.thisMachine = () => "relay:0123";
    await p.load();
    const now = BigInt(daemon.now);
    expect(p.view(now).rows.map((r) => r.state)).toEqual(["Running", "Paused"]);
    await p.send(new A.Pause({ name: "ada" }));
    expect(daemon.calls).toContain("agent_pause");
    expect(p.view(now).rows[0]!.state).toBe("Paused");
    await p.send(new A.Select({ name: "ada" }));
    expect(p.view(now).detail?.memory.map((l) => l.text)).toEqual(["hermes/MEMORY.md"]);
    await p.send(new A.AllowComputer({ machine: "relay:0123" }));
    expect(p.view(now).detail?.access.map((l) => l.text)).toEqual(["This computer"]);
    await p.send(new A.Resume({ name: "ada" }));
    expect(daemon.calls).toContain("agent_resume");
    expect(p.view(now).rows[0]!.state).toBe("Running");
  });

  it("pauses and resumes by name, one change at a time", async () => {
    const { bridge, model } = makeBridge();
    expect(await call(bridge, "agents.pause", { name: "ada" })).toBeNull();
    expect(daemon.paused).toBe(true);
    expect(await call(bridge, "agents.resume", { name: "ada" })).toBeNull();
    expect(daemon.paused).toBe(false);
    expect(shapeProblems("agents.pause", null)).toEqual([]);
    expect(await code(bridge, "agents.pause", {})).toBe("bad_args");
    model.agents.persistent.state = { ...model.agents.persistent.state, busy: true };
    expect(await message(bridge, "agents.pause", { name: "ada" })).toBe("Another change is still running");
  });

  it("says why a pause failed", async () => {
    const { bridge } = makeBridge();
    const fail = daemon.run;
    daemon.run = async (tool, args) => {
      if (tool === "agent_pause") throw new Error("the agent is not running");
      return fail(tool, args);
    };
    const { bridge: b2 } = makeBridge({ tools: (tool, args) => daemon.run(tool, args) });
    expect(await code(b2, "agents.pause", { name: "ada" })).toBe("failed");
    expect(await message(b2, "agents.pause", { name: "ada" })).toBe("the agent is not running");
    void bridge;
  });

  it("reads a run's events from the daemon, from a cursor", async () => {
    const { bridge } = makeBridge();
    const page = (await call(bridge, "agents.events", { spaceId: "local:dev", runId: "run-1", cursor: 7, max: 50 })) as Record<string, unknown>;
    expect(page).toMatchObject({ run_id: "run-1", cursor: 7, caught_up: true });
    expect(daemon.args.at(-1)).toEqual({ space: "local:dev", run_id: "run-1", cursor: 7, max: 50 });
    await call(bridge, "agents.events", { spaceId: "local:dev", runId: "run-1" });
    expect(daemon.args.at(-1)).toEqual({ space: "local:dev", run_id: "run-1", cursor: 0 });
    expect(shapeProblems("agents.events", page)).toEqual([]);
    expect(await code(bridge, "agents.events", { runId: "r" })).toBe("bad_args");
  });

  it("is unsupported without the daemon or the coding-agent setup", async () => {
    const { bridge } = makeBridge({ tools: null, setup: null });
    expect(await code(bridge, "agents.list")).toBe("unsupported");
    expect(await code(bridge, "agents.events", { spaceId: "s", runId: "r", cursor: 0 })).toBe("unsupported");
    expect(await code(bridge, "agents.setup")).toBe("unsupported");
    expect(await code(bridge, "agents.configure", {})).toBe("unsupported");
    expect(await code(bridge, "agents.setupDriver", { agents: ["codex"] })).toBe("unsupported");
    expect(await code(bridge, "agents.runs", {})).toBe("bad_args");
    expect(await code(bridge, "agents.pause", {})).toBe("bad_args");
  });

  // MARK: Coding agents on this machine (ViewModelTests, WebUIHostTests)

  it("answers the coding agents as the core's rows, and configures the ones asked for", async () => {
    const { bridge } = makeBridge();
    const rows = (await call(bridge, "agents.setup")) as { agent: string; configured: boolean; skillsTotal: number }[];
    expect(rows.map((r) => r.agent)).toEqual(["claude-code", "codex", "cursor"]);
    expect(rows[0]!.skillsTotal).toBe(2);
    expect(shapeProblems("agents.setup", rows)).toEqual([]);
    const configured = (await call(bridge, "agents.configure", { agents: ["claude-code"] })) as typeof rows;
    expect(setup.calls).toEqual(["setup:claude-code"]);
    expect(configured.find((r) => r.agent === "claude-code")?.configured).toBe(true);
    expect(shapeProblems("agents.configure", configured)).toEqual([]);
    expect(await code(bridge, "agents.configure", { agents: "codex" })).toBe("bad_args");
  });

  it("configures every detected agent when none are named", async () => {
    const { bridge } = makeBridge();
    await call(bridge, "agents.setup");
    await call(bridge, "agents.configure", {});
    expect(setup.calls).toEqual(["setup:claude-code,codex"]);
    await call(bridge, "agents.configure", { agents: null });
    expect(setup.calls).toEqual(["setup:claude-code,codex", "setup:claude-code,codex"]);
  });

  it("sets up the cua-driver skill and server for the agents named", async () => {
    const { bridge } = makeBridge();
    const outcomes = (await call(bridge, "agents.setupDriver", { agents: ["claude-code", "cursor"] })) as { agents: string[]; item: string }[];
    expect(setup.calls).toEqual(["driver:claude-code,cursor"]);
    expect(outcomes.map((o) => `${o.agents[0]} ${o.item}`)).toEqual(["claude-code cua-driver", "claude-code cua-driver"]);
    expect(shapeProblems("agents.setupDriver", outcomes)).toEqual([]);
    expect(await code(bridge, "agents.setupDriver", {})).toBe("bad_args");
  });

  it("presses a Settings row to configure it, and again to remove it, with a failure kept on its row", async () => {
    const { model } = makeBridge();
    const c = model.agents.coding;
    await c.reload();
    await c.press("agent:claude-code");
    expect(setup.calls).toEqual(["setup:claude-code"]);
    expect(c.rows?.find((r) => r.agent === "claude-code")?.configured).toBe(true);
    await c.press("agent:claude-code");
    expect(setup.calls.at(-1)).toBe("remove:claude-code");
    expect(c.rows?.find((r) => r.agent === "claude-code")?.configured).toBe(false);
    expect(c.pending).toEqual([]);
    await c.press("account");
    await c.press("agent:nobody");
    expect(setup.calls).toHaveLength(2);
    setup.setUp = async () => [{ agents: ["claude-code"], target: "mcp", item: "cua", change: "failed", detail: "permission denied" }];
    await c.press("agent:claude-code");
    const row = c.rows!.find((r) => r.agent === "claude-code")!;
    expect(row).toMatchObject({ configured: false, detail: "cua: permission denied" });
    setup.setUp = async () => {
      throw new Error("config is read-only");
    };
    await c.press("agent:codex");
    // Codex was configured, so the press removed it.
    expect(setup.calls.at(-1)).toBe("remove:codex");
  });

  it("does not configure everything twice at once, and reports a throw on every row", async () => {
    const { model } = makeBridge();
    const c = model.agents.coding;
    await c.reload();
    setup.setUp = async () => {
      throw new Error("no access");
    };
    const first = c.configureAll();
    await c.configureAll();
    await first;
    expect(c.busy).toBe(false);
    expect(c.rows!.filter((r) => r.installed).map((r) => r.detail)).toEqual(["no access", "no access"]);
  });

  it("registers the app's own cua as the MCP server, and counts the bundled skills", async () => {
    const seen: unknown[] = [];
    const info = { id: "codex", name: "Codex", installed: true, evidence: [], skillsDir: undefined, mcpConfig: undefined, mcpFormat: undefined, cuaConfigured: false, cuaManaged: false, cuaDriverConfigured: false, skillsInstalled: [], skillsOutdated: [], error: undefined } as unknown as AgentInfo;
    const outcome = (target: string, item: string): AgentSetupOutcome => ({ agents: ["codex"], target, item, path: "/x", change: "created", detail: "", backup: undefined }) as unknown as AgentSetupOutcome;
    const inner = {
      detect: () => [info],
      skills: () => [{}, {}, {}],
      installSkills: async (agents: string[], skills: string[], force: boolean) => (seen.push(["skills", agents, skills, force]), [outcome("skill", "cua-spaces")]),
      configureMcp: async (agents: string[], server: unknown) => (seen.push(["mcp", agents, server]), [outcome("mcp", "cua")]),
      setupCuaDriver: async (agents: string[], command: unknown, skills: boolean, mcp: boolean) => (seen.push(["driver", agents, command, skills, mcp]), []),
      remove: async (agents: string[], skills: boolean, mcp: boolean) => (seen.push(["remove", agents, skills, mcp]), []),
    } as unknown as AgentSetupLike;
    const live = new LiveAgentSetup(native, inner, "/Applications/Cua Spaces.app/Contents/MacOS/cua");
    expect(live.skillsTotal()).toBe(3);
    expect((await live.statuses()).map((s) => s.id)).toEqual(["codex"]);
    const done = await live.setUp(["codex"], true, true);
    expect(done.map((o) => `${o.target} ${o.item} ${o.change}`)).toEqual(["skill cua-spaces created", "mcp cua created"]);
    await live.setUpCuaDriver(["codex"]);
    await live.remove(["codex"]);
    expect(seen).toEqual([
      ["skills", ["codex"], [], false],
      ["mcp", ["codex"], { name: "cua", command: "/Applications/Cua Spaces.app/Contents/MacOS/cua", args: ["mcp"], env: new Map() }],
      ["driver", ["codex"], undefined, true, true],
      ["remove", ["codex"], true, true],
    ]);
    // A build without a bundled cua registers the default server.
    seen.length = 0;
    await new LiveAgentSetup(native, inner, null).setUp(["codex"], false, true);
    expect(seen).toEqual([["mcp", ["codex"], undefined]]);
  });

  // MARK: Provider keys (AgentKeysTests)

  it("saves and removes keys and never keeps the value", async () => {
    const { model } = makeBridge();
    const k = model.agents.keys;
    await k.load();
    expect(k.view.rows.map((r) => r.status)).toEqual(["Not set", "•••• 3f9a"]);
    expect(k.view.canEdit).toBe(true);
    expect(k.form("anthropic", null, "", false).canSave).toBe(false);
    await k.save("anthropic", "ANTHROPIC_API_KEY", "sk-ant-test-0000");
    // A provider key goes without a variable name; the daemon picks it.
    expect(keys.calls.at(-1)![0]).toBe("agent_keys.set");
    expect(keys.calls.at(-1)![1].env).toBeUndefined();
    expect(k.view.rows[0]!.status).toBe("•••• 0000");
    expect(k.view.rows[0]!.removeLabel).toBe("Remove");
    expect(k.removeConfirm("ANTHROPIC_API_KEY")?.title).toBe("Remove the Anthropic key?");
    expect(JSON.stringify(k.report)).not.toContain("sk-ant-test");
    await k.save("other", "MISTRAL_API_KEY", "mk-0000-9z9z");
    expect(keys.calls.at(-1)![1].env).toBe("MISTRAL_API_KEY");
    expect(k.view.rows.map((r) => r.title)).toEqual(["Anthropic", "OpenAI", "MISTRAL_API_KEY"]);
    await k.remove("ANTHROPIC_API_KEY");
    expect(k.view.rows[0]!.status).toBe("Not set");
  });

  it("has the core check the names of Other keys", () => {
    const { model } = makeBridge();
    const bad = model.agents.keys.form("other", null, "DYLD_INSERT_LIBRARIES", true);
    expect(bad.canSave).toBe(false);
    expect(bad.nameError).toContain("changes how programs run");
    const ok = model.agents.keys.form("other", null, "GEMINI_API_KEY", true);
    expect(ok.canSave).toBe(true);
    expect(ok.env).toBe("GEMINI_API_KEY");
  });

  it("says why keys can't be saved", async () => {
    keys.available = false;
    const { model } = makeBridge();
    await model.agents.keys.load();
    expect(model.agents.keys.view.canEdit).toBe(false);
    expect(model.agents.keys.view.notice).toContain("keeps credentials in a file");
    const none = makeBridge({ tools: null }).model.agents.keys;
    expect(none.view.notice).toContain("need the cua daemon");
    expect(agentKeysInput({ keys: [{ provider: "x", env: "", last4: "1234", added_ms: 1 }], available: false }, null)).toMatchObject({ keys: [], unavailable: "this machine can't keep keys" });
  });

  it("routes agentKeys.* to the daemon, with answers that never carry a key", async () => {
    const { bridge } = makeBridge();
    const listed = (await call(bridge, "agentKeys.list")) as { keys: { env: string }[] };
    expect(listed.keys.map((k) => k.env)).toEqual(["OPENAI_API_KEY"]);
    const set = await call(bridge, "agentKeys.set", { provider: "anthropic", value: "sk-ant-test-0000" });
    expect(keys.calls.at(-1)).toEqual(["agent_keys.set", { provider: "anthropic", value: "sk-ant-test-0000" }]);
    expect(JSON.stringify(set)).not.toContain("sk-ant");
    await call(bridge, "agentKeys.set", { provider: "other", env: "MISTRAL_API_KEY", value: "mk-0000-9z9z" });
    expect(keys.calls.at(-1)![1]).toEqual({ provider: "other", value: "mk-0000-9z9z", env: "MISTRAL_API_KEY" });
    const removed = await call(bridge, "agentKeys.remove", { env: "ANTHROPIC_API_KEY" });
    expect(keys.calls.at(-1)).toEqual(["agent_keys.remove", { env: "ANTHROPIC_API_KEY" }]);
    for (const [m, a] of [["agentKeys.list", listed], ["agentKeys.set", set], ["agentKeys.remove", removed]] as const) expect(shapeProblems(m, a)).toEqual([]);
    expect(await code(bridge, "agentKeys.set", { provider: "anthropic" })).toBe("bad_args");
    expect(await code(bridge, "agentKeys.remove", {})).toBe("bad_args");
  });

  it("fails a key call in the daemon's words, or without a daemon", async () => {
    const { bridge } = makeBridge({ tools: null });
    expect(await message(bridge, "agentKeys.list")).toBe("Agent keys need the cua daemon");
    expect(await code(bridge, "agentKeys.list")).toBe("failed");
  });

  // MARK: Events and telemetry

  it("tells the page when the agents or the coding agents' rows changed, once", async () => {
    const { bridge, model } = makeBridge();
    await tick(5);
    events.length = 0;
    await call(bridge, "agents.list");
    await tick(5);
    expect(events.filter((e) => e.event === "agents.changed")).toEqual([{ event: "agents.changed" }]);
    events.length = 0;
    await call(bridge, "agents.list");
    await tick(5);
    expect(events.filter((e) => e.event === "agents.changed")).toEqual([]);
    await call(bridge, "agents.pause", { name: "ada" });
    await tick(5);
    expect(events.filter((e) => e.event === "agents.changed")).toHaveLength(1);
    events.length = 0;
    await call(bridge, "agents.setup");
    await tick(5);
    expect(events.filter((e) => e.event === "agents.changed")).toHaveLength(1);
    expect(model.agents.coding.rows).not.toBeNull();
  });

  it("records the Agents page's open through the usage switch", async () => {
    const { bridge } = makeBridge();
    await call(bridge, "telemetry.track", { signals: [{ type: "feature", feature: "agents_page_open" }] });
    const last = telemetry.recorded.at(-1) as unknown as { tag: string; inner: { feature: string } };
    expect(last.tag).toBe("Feature");
    expect(last.inner.feature).toBe("agents_page_open");
  });
});
