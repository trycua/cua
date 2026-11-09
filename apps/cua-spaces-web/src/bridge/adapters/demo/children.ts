// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Child Spaces on demo data: every demo Space is top-level, except with
 * `?demo=network`, where research-swarm may make children, has two
 * workers, a budget with some of it used, and swarm-worker-1 keeps a web
 * server and a worker running; and `?demo=agent-home`, where the agent's
 * Home Space has made two children for its task (CUA-2630).
 */

import type { OpArgs, OpName, OpResult, ServiceWire, TreeBudgetLimitsWire, TreeBudgetWire, TreeNodeWire } from "../../protocol";
import type { SpaceRow } from "../../contracts/spaces";

type Handler<K extends OpName> = (args: OpArgs<K>) => Promise<OpResult<K>> | OpResult<K>;
type ChildOps = "spaces.tree" | "children.set" | "budget.get" | "budget.set" | "services.list" | "services.control";

const ROOT = "local:research-swarm";
export const AGENT_HOME_ID = "local:agent-home-demo";
const DEFAULTS = { maxSpaces: 16, maxVcpus: 16, maxMemoryMb: 32_768, usdPerHour: 2 };

export function createDemoChildren({
  rows,
  findRow,
  network,
  agentHome = false,
}: {
  rows: () => SpaceRow[];
  findRow: (id: string) => SpaceRow;
  /** `?demo=network`: research-swarm's tree. */
  network: boolean;
  /** `?demo=agent-home`: the agent's Home Space and the two Spaces it made. */
  agentHome?: boolean;
}): { [K in ChildOps]: Handler<K> } {
  const parents = new Map<string, string>([
    ...(network
      ? ([
          ["local:swarm-worker-1", ROOT],
          ["local:swarm-worker-2", ROOT],
        ] as [string, string][])
      : []),
    ...(agentHome
      ? ([
          ["local:acme-app-db", AGENT_HOME_ID],
          ["local:acme-app-build", AGENT_HOME_ID],
        ] as [string, string][])
      : []),
  ]);
  const on = new Set<string>([...(network ? [ROOT] : []), ...(agentHome ? [AGENT_HOME_ID] : [])]);
  let limits: TreeBudgetLimitsWire = network ? { usdPerHour: 1 } : {};
  const services = new Map<string, ServiceWire[]>(
    network
      ? [
          [
            "local:swarm-worker-1",
            [
              { name: "web", command: "python3 -m http.server 8080", state: "running", restarts: 0, lastEnd: "", probeError: "" },
              { name: "crawler", command: "./crawl.sh", state: "backoff", restarts: 3, lastEnd: "exited with code 1", probeError: "" },
            ],
          ],
        ]
      : [],
  );
  const rootOf = (id: string) => {
    let at = id;
    for (let i = 0; i < 8 && parents.has(at); i += 1) at = parents.get(at) as string;
    return at;
  };
  const budget = (id: string): TreeBudgetWire => {
    const root = rootOf(id);
    const kids = [...parents.entries()].filter(([, p]) => rootOf(p) === root).map(([c]) => c);
    const n = kids.length;
    return {
      root,
      set: limits,
      maxSpaces: limits.maxSpaces ?? DEFAULTS.maxSpaces,
      maxVcpus: limits.maxVcpus ?? DEFAULTS.maxVcpus,
      maxMemoryMb: limits.maxMemoryMb ?? DEFAULTS.maxMemoryMb,
      usdPerHour: limits.usdPerHour ?? DEFAULTS.usdPerHour,
      usdTotal: limits.usdTotal ?? null,
      spaces: n,
      vcpus: n * 2,
      memoryMb: n * 4096,
      usdPerHourUsed: 0,
      usdSpent: 0,
    };
  };
  const depth = (id: string) => {
    let d = 0;
    for (let at = id; parents.has(at) && d < 8; at = parents.get(at) as string) d += 1;
    return d;
  };
  return {
    "spaces.tree": (): TreeNodeWire[] =>
      rows().map((r) => ({
        id: r.id,
        parent: parents.get(r.id) ?? null,
        depth: depth(r.id),
        children: on.has(r.id),
        home: agentHome && r.id === AGENT_HOME_ID ? "claude-code" : null,
      })),
    "children.set": ({ spaceId, enabled }) => {
      findRow(spaceId);
      if (enabled) on.add(spaceId);
      else on.delete(spaceId);
      return null;
    },
    "budget.get": ({ spaceId }) => {
      findRow(spaceId);
      return budget(spaceId);
    },
    "budget.set": ({ spaceId, limits: next }) => {
      findRow(spaceId);
      limits = { ...next };
      return budget(spaceId);
    },
    "services.list": ({ spaceId }) => {
      findRow(spaceId);
      return services.get(spaceId) ?? [];
    },
    "services.control": ({ spaceId, name, action }) => {
      findRow(spaceId);
      const list = services.get(spaceId) ?? [];
      services.set(
        spaceId,
        action === "remove"
          ? list.filter((s) => s.name !== name)
          : list.map((s) =>
              s.name === name ? { ...s, state: action === "stop" ? "stopped" : "running", restarts: action === "restart" ? s.restarts + 1 : s.restarts } : s,
            ),
      );
      return null;
    },
  };
}
