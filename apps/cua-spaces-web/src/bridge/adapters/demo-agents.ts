// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Demo agents, in the hosts' wire shapes: four persistent agents (Claude
 * Code, Codex, Hermes, OpenClaw), five runs in the demo's running Spaces,
 * and each run's events as `agent_events` reports them. One run is live:
 * its third turn is written over about twenty-five seconds after the
 * adapter starts, so the timeline streams. All synthetic.
 */

import type { AgentEvent, AgentRunStatus, AgentSetupRow, PersistentAgent, SpaceAgentRun } from "../contracts/agents";

const MIN = 60_000;

export function demoPersistentAgents(now: number): PersistentAgent[] {
  return [
    { name: "ada", harness: "claude-code", space: "local:design-review", paused: false, spaceState: "running", runId: "run-ada-7", savedMs: now - 2 * MIN },
    { name: "atlas", harness: "openai-codex", space: "relay:linux-box/ubuntu-build", paused: false, spaceState: "running", runId: null, savedMs: now - 38 * MIN },
    { name: "claw", harness: "openclaw", space: "relay:mac-mini/qa-windows", paused: false, spaceState: "running", runId: null, savedMs: now - 3 * 60 * MIN },
    {
      name: "scout",
      harness: "hermes",
      space: "relay:mac-mini/release-checks",
      paused: true,
      spaceState: "released",
      runId: null,
      savedMs: now - 26 * 60 * MIN,
    },
  ];
}

export function demoAgentSetup(): AgentSetupRow[] {
  const row = (agent: string, name: string, installed: boolean, configured: boolean, mcpConfig: string | null, skillsDir: string | null): AgentSetupRow => ({
    agent,
    name,
    installed,
    configured,
    detail: !installed ? "Not installed" : configured ? "Skills and MCP server set up" : "Not set up",
    skillsInstalled: configured ? 6 : 0,
    skillsTotal: 6,
    mcpConfig,
    skillsDir,
  });
  return [
    row("claude-code", "Claude Code", true, true, "~/.claude.json", "~/.claude/skills"),
    row("codex", "OpenAI Codex", true, true, "~/.codex/config.toml", "~/.agents/skills"),
    row("hermes", "Hermes", true, false, "~/.hermes/config.yaml", "~/.hermes/skills"),
    row("openclaw", "OpenClaw", true, false, "~/.openclaw/openclaw.json", "~/.agents/skills"),
    row("cursor", "Cursor", true, true, "~/.cursor/mcp.json", "~/.cursor/skills"),
    row("gemini-cli", "Gemini CLI", false, false, "~/.gemini/settings.json", "~/.gemini/skills"),
  ];
}

/* ---- Event scripts ---------------------------------------------------------- */

/** Builds one run's events the way the runner writes them (summaries included). */
class Script {
  readonly events: AgentEvent[] = [];
  private seq = 0;
  private turn = 0;
  private tools = 0;
  constructor(private t: number) {}

  private push(e: Omit<AgentEvent, "seq" | "ts_ms" | "turn">, gapMs: number): this {
    this.t += gapMs;
    this.events.push({ seq: ++this.seq, ts_ms: this.t, turn: this.turn, ...e });
    return this;
  }

  /** Sets the clock. */
  at(t: number): this {
    this.t = t;
    return this;
  }

  /** Moves the clock without writing anything. */
  wait(ms: number): this {
    this.t += ms;
    return this;
  }

  prompt(text: string): this {
    this.turn += 1;
    return this.push({ kind: "turn_started", text, category: "user" }, 400);
  }

  thought(text: string, gapMs = 900): this {
    return this.push({ kind: "thought", text, category: "activity", summary: `Thinking: ${text.split("\n")[0]}` }, gapMs);
  }

  tool(kind: string, title: string, status: "completed" | "failed" = "completed", output?: string, ms = 1400): this {
    const id = `toolu_${String(++this.tools).padStart(2, "0")}`;
    this.push({ kind: "tool_call", tool_id: id, tool_title: title, tool_kind: kind, tool_status: "pending", category: "activity", summary: `Tool ${title}` }, 500);
    const tail = output ? `: ${output.split("\n")[0]}` : "";
    return this.push(
      { kind: "tool_update", tool_id: id, tool_title: title, tool_kind: kind, tool_status: status, text: output, category: "activity", summary: `Tool ${title} ${status}${tail}` },
      ms,
    );
  }

  /** The agent's words, in the small chunks a harness streams. */
  say(markdown: string, chunkGapMs = 70): this {
    const words = markdown.match(/\S+\s*|\s+/g) ?? [];
    for (let i = 0; i < words.length; ) {
      const n = 2 + ((i * 7) % 4);
      this.push({ kind: "message", text: words.slice(i, i + n).join(""), category: "message" }, i === 0 ? 600 : chunkGapMs);
      i += n;
    }
    return this;
  }

  end(stopReason = "end_turn"): this {
    return this.push(
      { kind: "turn_ended", stop_reason: stopReason, category: "activity", summary: `Turn ${this.turn} ended (${stopReason})` },
      300,
    );
  }

  error(text: string): this {
    return this.push({ kind: "error", text, category: "activity", summary: `Error: ${text}` }, 200);
  }

  usage(): this {
    return this.push({ kind: "usage", category: "hidden" }, 50);
  }
}

export interface DemoRunRecord {
  space: string;
  run: SpaceAgentRun;
  events: AgentEvent[];
  /** The run is running until its last event is written. */
  live: boolean;
  /** Set when a pause cut the run short. */
  stoppedAt?: number;
}

const ONBOARDING_DIFF = `## Onboarding copy

I kept the structure and only changed the words. Three screens change:

1. **Welcome**: one sentence instead of two, and no exclamation mark.
2. **Sign in**: says what signing in is for before asking.
3. **Agents**: names the MCP server once, then refers to it as "the server".

\`\`\`diff
- <h1>Welcome to Cua Spaces!</h1>
- <p>Let's get you set up. It only takes a minute!</p>
+ <h1>Welcome to Cua Spaces</h1>
+ <p>Set up takes about a minute.</p>

- <p>Sign in to sync your Spaces across all your devices!</p>
+ <p>Sign in to see your Spaces on every machine you use.</p>
\`\`\`

The longest sentence went from 31 words to 14. Nothing else in \`routes/onboarding\` reads the old strings, and \`pnpm test\` still passes.

Want me to commit this as \`onboarding: shorter copy\`, or keep going on the Settings page first?`;

function adaRun(t0: number): DemoRunRecord {
  const s = new Script(t0 - 52 * MIN)
    .prompt("Look at the Spaces grid in the web UI. Tiles overflow their column at 1280 px wide. Find out why.")
    .thought("The grid uses auto-fill with a 232 px minimum, so the tile itself must refuse to shrink.")
    .tool("read", "Read apps/cua-spaces-web/src/routes/spaces/index.tsx")
    .tool("execute", 'rg -n "shrink|min-w" apps/cua-spaces-web/src/components', "completed", "3 matches in 2 files")
    .say(
      "The overflow comes from the state label. `StateLabel` sits in a flex row next to the Space name, and it has no `shrink-0`, so a long machine name pushes it out of the tile instead of truncating.\n\nThe name already has `truncate`, but its wrapper needs `min-w-0` for that to work inside flex:\n\n```tsx\n<div className=\"min-w-0 flex-1\">\n  <div className=\"truncate\">{space.name}</div>\n</div>\n```\n\nI haven't changed anything yet.",
    )
    .usage()
    .end()
    .wait(6 * MIN)
    .prompt("Fix it and add a test.")
    .tool("edit", "Edit apps/cua-spaces-web/src/routes/spaces/index.tsx")
    .tool("edit", "Edit apps/cua-spaces-web/src/lib/spaces.test.ts")
    .tool("execute", "pnpm test -- spaces", "completed", "12 passed")
    .say(
      "Fixed. The name column is `min-w-0 flex-1` and the label is `shrink-0`, so long names truncate and the label stays put.\n\n- `routes/spaces/index.tsx`: two class changes\n- `lib/spaces.test.ts`: a case with a 60-character machine name\n\nAll 12 tests pass.",
    )
    .usage()
    .end()
    .wait(40 * MIN)
    .prompt("Now tighten the onboarding copy: shorter sentences, no exclamation marks. Show me the diff before you commit.")
    .thought("Read the onboarding route first, then the strings it imports.")
    .tool("read", "Read apps/cua-spaces-web/src/routes/onboarding/index.tsx")
    // Everything above was written before the demo started; what follows is
    // written live, starting a moment after the adapter starts.
    .at(t0 + 1_000)
    .tool("execute", "rg -n '!' apps/cua-spaces-web/src/routes/onboarding", "completed", "4 matches in 1 file", 2200)
    .thought("Four exclamation marks, all in the first two screens. The agents screen repeats \"MCP server\" three times.", 1600)
    .tool("edit", "Edit apps/cua-spaces-web/src/routes/onboarding/index.tsx", "completed", undefined, 2600)
    .tool("execute", "pnpm test -- onboarding", "completed", "8 passed", 3200)
    .say(ONBOARDING_DIFF, 180)
    .usage()
    .end();
  const events = s.events;
  return {
    space: "local:design-review",
    live: true,
    events,
    run: {
      runId: "run-ada-7",
      agent: "claude-code",
      status: "running",
      reason: "a turn is running",
      summary: "Tidy the Spaces grid and the onboarding copy",
      createdAt: Math.floor((t0 - 52 * MIN) / 1000),
      phase: "tool",
      turn: 3,
    },
  };
}

function finished(space: string, run: Omit<SpaceAgentRun, "turn" | "phase">, script: Script, phase = "waiting"): DemoRunRecord {
  const turn = script.events.reduce((n, e) => Math.max(n, e.turn), 0);
  return { space, live: false, events: script.events, run: { ...run, turn, phase } };
}

export function demoRunRecords(t0: number): DemoRunRecord[] {
  const snapshots = new Script(t0 - 2 * 60 * MIN)
    .prompt("Update the Playwright snapshots for the dark theme. Only the ones that changed because of the new tokens.")
    .tool("execute", "pnpm exec playwright test --grep @dark", "completed", "6 failed, 8 passed")
    .thought("All six failures are colour diffs on the sidebar and the top bar. Nothing moved.")
    .tool("execute", "pnpm exec playwright test --grep @dark --update-snapshots", "completed", "14 passed, 6 updated")
    .say(
      "Updated 6 dark-theme snapshots. Every diff is the sidebar and top bar picking up the new `--chrome` colour; no layout changed.\n\nI left the light snapshots alone.",
    )
    .end();

  const bisect = new Script(t0 - 95 * MIN)
    .prompt("Bisect the flaky reconnect test in cua-spacesd. It fails about one run in five on the Linux box.")
    .tool("execute", "cargo test -p cua-spacesd reconnect -- --nocapture", "failed", "1 failed: reconnect_after_suspend timed out after 5s")
    .thought("Run it in a loop to get a reliable signal before bisecting.")
    .tool("execute", "for i in $(seq 20); do cargo test -q -p cua-spacesd reconnect || echo FAIL; done", "completed", "4 of 20 failed")
    .tool("execute", "git bisect run scripts/flaky.sh 20", "completed", "first bad commit: 6c1e2a4", 4200)
    .say(
      "The first bad commit is `6c1e2a4` (\"spacesd: reuse the stream socket across suspends\").\n\nAfter a suspend, the reconnect races the old socket's close. When the close lands second, the new stream reads the old `EOF` and gives up:\n\n```rust\n// stream.rs, after resume\nlet sock = self.sock.take().unwrap_or_else(|| connect(addr))?;\n```\n\nDropping the old socket before resuming fixes it: 0 failures in 100 runs.",
    )
    .end()
    .wait(12 * MIN)
    .prompt("Write the fix up as a draft commit. Don't push.")
    .tool("edit", "Edit libs/cua-spacesd/src/stream.rs")
    .tool("execute", 'git commit -am "spacesd: drop the old stream socket before resuming"', "completed", "[fix/reconnect 9d03b1f]")
    .say("Committed as `9d03b1f` on `fix/reconnect`. Nothing is pushed.")
    .end();

  const deb = new Script(t0 - 4 * 60 * MIN)
    .prompt("Build the arm64 .deb and attach it to the draft release.")
    .tool("execute", "dpkg-buildpackage -a arm64 -us -uc", "failed", "unmet build dependencies: libssl-dev:arm64", 5200)
    .say("The build stopped before compiling: `libssl-dev:arm64` isn't installed in this Space, and I can't install packages here without sudo.")
    .error("the runner exited with code 2");

  const installer = new Script(t0 - 3 * 60 * MIN)
    .prompt("Install the 0.6.1 Windows build and note anything that breaks.")
    .tool("execute", "msiexec /i CuaSpaces-0.6.1.msi /qn /l*v install.log", "completed", "exit code 0", 6400)
    .tool("read", "Read %LOCALAPPDATA%\\Cua\\logs\\spaces.log")
    .say(
      "It installs and starts. Two things to look at:\n\n1. The first launch shows the window for about a second before the sign-in sheet, unstyled.\n2. `spaces.log` warns that the hotkey `Ctrl+Shift+Space` is already taken by another app. The app doesn't say so anywhere.\n\nNothing crashed.",
    )
    .end();

  return [
    adaRun(t0),
    finished(
      "local:design-review",
      { runId: "run-3f1c", agent: "openai-codex", status: "idle", reason: "waiting for a follow-up", summary: "Update the Playwright snapshots for the dark theme", createdAt: Math.floor((t0 - 2 * 60 * MIN) / 1000) },
      snapshots,
    ),
    finished(
      "relay:linux-box/ubuntu-build",
      { runId: "run-atlas-12", agent: "openai-codex", status: "idle", reason: "waiting for a follow-up", summary: "Bisect the flaky reconnect test in cua-spacesd", createdAt: Math.floor((t0 - 95 * MIN) / 1000) },
      bisect,
    ),
    finished(
      "relay:linux-box/ubuntu-build",
      { runId: "run-9b20", agent: "hermes", status: "failed", reason: "the runner exited with code 2", summary: "Build the arm64 .deb and attach it to the draft release", createdAt: Math.floor((t0 - 4 * 60 * MIN) / 1000) },
      deb,
      "exited",
    ),
    finished(
      "relay:mac-mini/qa-windows",
      { runId: "run-claw-4", agent: "openclaw", status: "idle", reason: "waiting for a follow-up", summary: "Install the 0.6.1 Windows build and note what breaks", createdAt: Math.floor((t0 - 3 * 60 * MIN) / 1000) },
      installer,
    ),
  ];
}

/** The events written by `now`. */
export function writtenEvents(rec: DemoRunRecord, now: number): AgentEvent[] {
  const until = rec.stoppedAt ?? now;
  return rec.events.filter((e) => e.ts_ms <= until);
}

/** A record's run as `list_space_agents` would report it at `now`. */
export function runAt(rec: DemoRunRecord, now: number): SpaceAgentRun {
  if (!rec.live) return rec.run;
  const written = writtenEvents(rec, now);
  const last = written[written.length - 1];
  const done = rec.stoppedAt !== undefined || written.length === rec.events.length;
  const status: AgentRunStatus = done ? "idle" : "running";
  return {
    ...rec.run,
    status,
    reason: done ? "waiting for a follow-up" : "a turn is running",
    phase: done ? "waiting" : last?.kind === "message" ? "responding" : "tool",
  };
}
