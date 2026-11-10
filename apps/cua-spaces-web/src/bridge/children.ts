// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Child Spaces from the host (`spaces.tree`, `children.set`, `budget.get`,
 * `budget.set`, `services.list`, `services.control`), behind the store the
 * Spaces page and the Space page read ([`ChildrenPreview`]). A host that
 * does not answer them leaves every read empty, and none of it shows.
 *
 * Reads are cached and refreshed when asked again after a while; a change
 * reads again at once.
 */

import { isUnsupported, type DataAdapter } from "./adapter";
import type { ChildrenPreview, KeptService, TreeBudget, TreeBudgetLimits, TreeNode } from "./contracts/children";
import type { ServiceWire, TreeBudgetLimitsWire, TreeBudgetWire, TreeNodeWire } from "./protocol";

/** How long a read stays fresh. */
const FRESH_MS = 10_000;

const num = (v: number | null | undefined) => (v === null || v === undefined ? undefined : v);

export function nodeFromWire(w: TreeNodeWire): TreeNode {
  return { id: w.id, parent: w.parent ?? undefined, depth: w.depth, childrenOn: w.children, ...(w.home ? { home: w.home } : {}) };
}

export function limitsFromWire(w: TreeBudgetLimitsWire | null | undefined): TreeBudgetLimits {
  return {
    maxSpaces: num(w?.maxSpaces),
    maxVcpus: num(w?.maxVcpus),
    maxMemoryMb: num(w?.maxMemoryMb),
    usdPerHour: num(w?.usdPerHour),
    usdTotal: num(w?.usdTotal),
  };
}

export function budgetFromWire(w: TreeBudgetWire): TreeBudget {
  return { ...w, set: limitsFromWire(w.set), usdTotal: num(w.usdTotal) };
}

export function serviceFromWire(w: ServiceWire): KeptService {
  return { name: w.name, command: w.command, state: w.state, restarts: w.restarts, lastEnd: w.lastEnd, probeError: w.probeError };
}

/** The store over `adapter`'s operations. */
export function createChildrenStore(adapter: DataAdapter, now: () => number = Date.now): ChildrenPreview {
  const listeners = new Set<() => void>();
  let version = 0;
  let unsupported = false;
  const changed = () => {
    version += 1;
    for (const l of listeners) l();
  };
  const failed = (e: unknown) => {
    if (isUnsupported(e)) unsupported = true;
  };
  const errors = new Map<string, string>();
  const refused = (spaceId: string) => (e: unknown) => {
    failed(e);
    errors.set(spaceId, e instanceof Error ? e.message : String(e));
    changed();
  };

  // The tree, for every Space at once.
  let nodes = new Map<string, TreeNode>();
  let treeAt = 0;
  let treeLoading = false;
  const readTree = () => {
    if (unsupported || treeLoading) return;
    treeLoading = true;
    void adapter
      .call("spaces.tree", {})
      .then((list) => {
        nodes = new Map(list.map((w) => [w.id, nodeFromWire(w)]));
      })
      .catch(failed)
      .finally(() => {
        treeAt = now();
        treeLoading = false;
        changed();
      });
  };
  const tree = () => {
    if (now() - treeAt > FRESH_MS) readTree();
    return nodes;
  };

  // Budgets by Space asked about, services by Space.
  const cached = <T>(load: (id: string) => Promise<T>, empty: T) => {
    const values = new Map<string, { value: T; at: number; loading: boolean }>();
    const read = (id: string) => {
      const e = values.get(id) ?? { value: empty, at: 0, loading: false };
      values.set(id, e);
      if (unsupported || e.loading) return;
      e.loading = true;
      void load(id)
        .then((v) => {
          e.value = v;
        })
        .catch(failed)
        .finally(() => {
          e.at = now();
          e.loading = false;
          changed();
        });
    };
    return {
      get(id: string): T {
        const e = values.get(id);
        if (!e || now() - e.at > FRESH_MS) read(id);
        return values.get(id)?.value ?? empty;
      },
      reread(id: string) {
        const e = values.get(id);
        if (e) e.at = 0;
        read(id);
      },
    };
  };
  const budgets = cached<TreeBudget | null>(async (id) => budgetFromWire(await adapter.call("budget.get", { spaceId: id })), null);
  const services = cached<KeptService[]>(async (id) => (await adapter.call("services.list", { spaceId: id })).map(serviceFromWire), []);

  return {
    node: (spaceId) => (unsupported ? null : (tree().get(spaceId) ?? null)),
    childrenOf: (spaceId) =>
      unsupported
        ? []
        : [...tree().values()]
            .filter((n) => n.parent === spaceId)
            .map((n) => n.id)
            .sort(),
    homesOf: (agent) =>
      unsupported
        ? []
        : [...tree().values()]
            .filter((n) => n.home !== undefined)
            .sort((a, b) => Number(b.home === agent) - Number(a.home === agent) || a.id.localeCompare(b.id))
            .map((n) => n.id),
    setChildrenOn(spaceId, on) {
      const n = nodes.get(spaceId);
      if (n) nodes.set(spaceId, { ...n, childrenOn: on });
      changed();
      void adapter
        .call("children.set", { spaceId, enabled: on })
        .then(() => errors.delete(spaceId))
        .catch(refused(spaceId))
        .finally(() => {
          treeAt = 0;
          readTree();
        });
    },
    budget: (spaceId) => (unsupported ? null : budgets.get(spaceId)),
    setBudget(spaceId, limits) {
      void adapter
        .call("budget.set", { spaceId, limits })
        .then(() => errors.delete(spaceId))
        .catch(refused(spaceId))
        .finally(() => budgets.reread(spaceId));
    },
    services: (spaceId) => (unsupported ? [] : services.get(spaceId)),
    controlService(spaceId, name, action) {
      void adapter
        .call("services.control", { spaceId, name, action })
        .then(() => errors.delete(spaceId))
        .catch(refused(spaceId))
        .finally(() => services.reread(spaceId));
    },
    error: (spaceId) => errors.get(spaceId) ?? null,
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    version: () => version,
  };
}
