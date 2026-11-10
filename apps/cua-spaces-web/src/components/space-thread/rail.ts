// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * What the call layout's right rail shows of the core's lists before
 * Details › More: the page draws with these, and the parity flows check
 * the rail against the same rule (e2e/parity.spec.ts).
 */

/** The fact fields the rail reads. */
interface Fact {
  label: string;
}

/** The facts the rail shows before More: the first three the header does not already say. */
export function panelFacts<F extends Fact>(facts: readonly F[]): F[] {
  return facts.filter((f) => f.label !== "Status" && f.label !== "Identifier").slice(0, 3);
}

/** Windows the rail's Stream lists before "Show all". */
export const RAIL_WINDOWS = 4;

/** The windows the rail lists: the core's window rows (the desktop is the stage), the first few. */
export function railWindows<R extends { kind: string }>(rows: readonly R[]): R[] {
  return rows.filter((r) => r.kind === "window").slice(0, RAIL_WINDOWS);
}

/** The windows the rail lists with one picked for the main view: the first few, and the picked one too when it is further down. */
export function railStreams<R extends { kind: string; id: string }>(rows: readonly R[], selected: string | null): R[] {
  const first = railWindows(rows);
  const picked = selected ? rows.find((r) => r.kind === "window" && r.id === selected) : undefined;
  return picked && !first.includes(picked) ? [...first, picked] : first;
}
