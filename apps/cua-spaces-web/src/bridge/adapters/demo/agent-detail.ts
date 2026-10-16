// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's persistent-agent detail: each demo agent's home files
 * (with a few versions), routines, computer grants and access audit, in
 * memory, answered as the native hosts answer `agents.detail` and
 * `agents.readFile`. This Mac is `relay:this-mac`. All synthetic.
 */

import { HostError } from "../../adapter";
import type { AgentAccessAudit, AgentComputerGrant, AgentFileVersion, AgentRoutine } from "../../contracts/agents";
import type { AgentDetailOp } from "../../ops/agent-detail";
import type { DemoHandlers } from "./context";

const MIN = 60_000;
const HOUR = 60 * MIN;
const THIS_MACHINE = "relay:this-mac";

interface DemoFile {
  text: string;
  /** Older texts, newest first (`versions[0]` is the one before the current). */
  history: { text: string; at: number }[];
  at: number;
}

const DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const WEEKDAYS = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/** The daemon's routine label (`cua_spaces::routines::Schedule::label`). */
function clock(time: string): string {
  const [h = 0, m = 0] = time.split(":").map(Number);
  return `${h % 12 === 0 ? 12 : h % 12}:${String(m).padStart(2, "0")} ${h < 12 ? "AM" : "PM"}`;
}

export function routineLabel(everyMinutes: number | null, dailyAt: string | null, weeklyOn: string | null): string {
  if (everyMinutes !== null) {
    if (everyMinutes === 1) return "Every minute";
    if (everyMinutes === 60) return "Every hour";
    if (everyMinutes % 60 === 0) return `Every ${everyMinutes / 60} hours`;
    return `Every ${everyMinutes} minutes`;
  }
  if (dailyAt) return `Every day at ${clock(dailyAt)}`;
  const [day = "mon", time = "09:00"] = (weeklyOn ?? "").split(" ");
  return `Every ${DAYS[Math.max(0, WEEKDAYS.indexOf(day))]} at ${clock(time)}`;
}

function demoHomes(now: number): Map<string, Map<string, DemoFile>> {
  const file = (text: string, at: number, ...history: [string, number][]): DemoFile => ({ text, at, history: history.map(([t, a]) => ({ text: t, at: a })) });
  return new Map([
    [
      "ada",
      new Map([
        [
          "MEMORY.md",
          file(
            "# Ada\n\n- The design review runs in local:design-review.\n- Francesco wants screenshots in light and dark.\n- Figma exports go to public/design/.\n",
            now - 2 * MIN,
            ["# Ada\n\n- The design review runs in local:design-review.\n- Figma exports go to public/design/.\n", now - 3 * HOUR],
            ["# Ada\n\n- The design review runs in local:design-review.\n", now - 2 * 24 * HOUR],
          ),
        ],
        ["notes/review-checklist.md", file("1. Contrast in both themes\n2. Keyboard focus on every control\n3. Copy matches the native app\n", now - 50 * MIN)],
        ["notes/open-questions.md", file("- Should the notch show routines?\n", now - 26 * HOUR)],
      ]),
    ],
    [
      "atlas",
      new Map([
        ["MEMORY.md", file("# Atlas\n\nBuilds the Linux images in relay:linux-box/ubuntu-build.\n", now - 38 * MIN, ["# Atlas\n", now - 5 * 24 * HOUR])],
        ["runbooks/nightly-build.md", file("cargo build --release, then upload the artifacts to public/builds/.\n", now - 9 * HOUR)],
      ]),
    ],
    ["claw", new Map([["MEMORY.md", file("# Claw\n\nRuns the Windows QA pass.\n", now - 3 * HOUR)]])],
    ["scout", new Map([["MEMORY.md", file("# Scout\n\nChecks each release candidate before it ships.\n", now - 26 * HOUR)]])],
  ]);
}

function demoRoutines(): Map<string, AgentRoutine[]> {
  return new Map([
    [
      "ada",
      [
        { id: "rt-ada-standup", title: "Morning design notes", label: "Every day at 9:00 AM", enabled: true },
        { id: "rt-ada-weekly", title: "Weekly screenshot sweep", label: "Every Friday at 4:00 PM", enabled: false },
      ],
    ],
    ["atlas", [{ id: "rt-atlas-nightly", title: "Nightly build", label: "Every day at 2:00 AM", enabled: true }]],
    ["claw", []],
    ["scout", [{ id: "rt-scout-rc", title: "Check the latest release candidate", label: "Every 6 hours", enabled: true }]],
  ]);
}

export function demoAgentDetailHandlers({ wait, stepMs, now }: { wait(ms: number): Promise<void>; stepMs: number; now: () => number }): DemoHandlers<AgentDetailOp> {
  const homes = demoHomes(now());
  const routines = demoRoutines();
  let grants: AgentComputerGrant[] = [
    { agent: "ada", machine: THIS_MACHINE, revoked: false },
    { agent: "atlas", machine: "relay:linux-box", revoked: false },
  ];
  const audit: AgentAccessAudit[] = [
    { tsMs: now() - 4 * MIN, action: "use", principal: "agent:ada", path: THIS_MACHINE, detail: "screenshot" },
    { tsMs: now() - 3 * HOUR, action: "grant", principal: "user", path: THIS_MACHINE, detail: "agent:ada" },
    { tsMs: now() - 7 * HOUR, action: "use", principal: "agent:atlas", path: "relay:linux-box", detail: "shell" },
    { tsMs: now() - 2 * 24 * HOUR, action: "denied", principal: "agent:claw", path: THIS_MACHINE, detail: "not allowed" },
  ];
  let nextRoutine = 1;

  const split = (path: string): [string, string] => {
    const m = /^agents\/([^/]+)\/(.+)$/.exec(path);
    if (!m) throw new HostError(`not found: ${path}`, "not_found");
    return [m[1]!, m[2]!];
  };
  const fileAt = (path: string) => {
    const [agent, rel] = split(path);
    const f = homes.get(agent)?.get(rel);
    if (!f) throw new HostError(`not found: ${path}`, "not_found");
    return f;
  };
  const versions = (path: string, f: DemoFile): AgentFileVersion[] => [
    { version: `${path}@0`, modifiedMs: f.at, deleted: false, latest: true },
    ...f.history.map((h, i) => ({ version: `${path}@${i + 1}`, modifiedMs: h.at, deleted: false, latest: false })),
  ];
  const audited = (action: string, agent: string, machine: string) =>
    audit.unshift({ tsMs: now(), action, principal: "user", path: machine, detail: `agent:${agent}` });

  return {
    "agents.detail": async ({ name }) => {
      await wait(stepMs / 4);
      const home = [...(homes.get(name) ?? new Map<string, DemoFile>()).entries()].map(([rel, f]) => ({
        path: `agents/${name}/${rel}`,
        name: rel.split("/").pop()!,
        size: new TextEncoder().encode(f.text).length,
      }));
      return {
        home,
        routines: (routines.get(name) ?? []).map((r) => ({ ...r })),
        grants: grants.map((g) => ({ ...g })),
        audit: audit.map((a) => ({ ...a })),
        thisMachine: THIS_MACHINE,
      };
    },
    "agents.readFile": async ({ path }) => {
      await wait(stepMs / 4);
      const f = fileAt(path);
      return { path, text: f.text, binary: false, versions: versions(path, f) };
    },
    "agents.restoreFile": async ({ path, version }) => {
      const f = fileAt(path);
      const i = Number(version.split("@").pop());
      const old = f.history[i - 1];
      if (!old) throw new HostError(`not found: version ${version}`, "not_found");
      await wait(stepMs / 2);
      f.history.unshift({ text: f.text, at: f.at });
      f.text = old.text;
      f.at = now();
      return null;
    },
    "agents.addRoutine": async ({ agent, title, prompt, everyMinutes, dailyAt, weeklyOn }) => {
      if (!title.trim() || !prompt.trim()) throw new HostError("a routine needs a title and a prompt", "bad_args");
      await wait(stepMs / 2);
      const list = routines.get(agent) ?? [];
      routines.set(agent, [...list, { id: `rt-${agent}-new-${nextRoutine++}`, title, label: routineLabel(everyMinutes ?? null, dailyAt ?? null, weeklyOn ?? null), enabled: true }]);
      return null;
    },
    "agents.setRoutine": async ({ id, enabled }) => {
      await wait(stepMs / 4);
      for (const [agent, list] of routines) routines.set(agent, list.map((r) => (r.id === id ? { ...r, enabled } : r)));
      return null;
    },
    "agents.removeRoutine": async ({ id }) => {
      await wait(stepMs / 4);
      for (const [agent, list] of routines) routines.set(agent, list.filter((r) => r.id !== id));
      return null;
    },
    // The daemon asks for Touch ID first; the demo takes a moment instead.
    "agents.allowComputer": async ({ agent, machine }) => {
      await wait(stepMs);
      grants = [...grants.filter((g) => !(g.agent === agent && g.machine === machine)), { agent, machine, revoked: false }];
      audited("grant", agent, machine);
      return null;
    },
    "agents.revokeComputer": async ({ agent, machine }) => {
      await wait(stepMs / 4);
      grants = grants.map((g) => (g.agent === agent && g.machine === machine ? { ...g, revoked: true } : g));
      audited("revoke", agent, machine);
      return null;
    },
  };
}
