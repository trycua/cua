// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { SIZES } from "./fixtures";
import { getTemplate } from "./templates";
import type {
  Fleet,
  FleetDraft,
  HourlyEstimate,
  MachineSize,
  Space,
  SpaceOs,
  TemplateMember,
} from "./types";

/**
 * Synthetic OS multipliers on top of the Linux list price. Windows and macOS
 * carry licensing overhead; these numbers are illustrative only.
 */
const OS_MULTIPLIER: Record<SpaceOs, number> = {
  linux: 1,
  windows: 1.35,
  macos: 2.1,
};

export const MIN_FLEET_SIZE = 1;
export const MAX_FLEET_SIZE = 8;

export function clampCount(count: number): number {
  if (!Number.isFinite(count)) return MIN_FLEET_SIZE;
  return Math.min(MAX_FLEET_SIZE, Math.max(MIN_FLEET_SIZE, Math.round(count)));
}

/** Members the draft would provision, honouring the adjustable count. */
export function resolveMembers(draft: FleetDraft): TemplateMember[] {
  const template = getTemplate(draft.templateId);
  if (!template.adjustable) return template.members;

  const count = clampCount(draft.count);
  const members: TemplateMember[] = template.members.slice(0, count);
  const base = template.members[0];
  for (let i = members.length; i < count; i += 1) {
    members.push(
      base
        ? { ...base, name: `${stripIndex(base.name)} ${i + 1}` }
        : { name: `Computer ${i + 1}`, os: draft.customOs, scene: sceneFor(draft.customOs) },
    );
  }
  return members;
}

function stripIndex(name: string): string {
  return name.replace(/\s+[A-Z0-9]$/i, "");
}

function sceneFor(os: SpaceOs): TemplateMember["scene"] {
  switch (os) {
    case "windows":
      return "windows-desktop";
    case "macos":
      return "mac-desktop";
    case "linux":
      return "linux-terminal";
  }
}

export function hourlyRate(os: SpaceOs, size: MachineSize): number {
  const option = SIZES.find((s) => s.id === size);
  if (!option) throw new Error(`Unknown size: ${size}`);
  return round2(option.linuxHourlyUsd * OS_MULTIPLIER[os]);
}

/**
 * Upper-bound hourly estimate: every computer running for a full hour.
 * Auto-suspend can only lower the real figure, so the UI labels this "up to".
 */
export function estimateHourly(draft: FleetDraft): HourlyEstimate {
  const members = resolveMembers(draft);
  const byOs = new Map<SpaceOs, number>();
  for (const m of members) byOs.set(m.os, (byOs.get(m.os) ?? 0) + 1);

  const breakdown = [...byOs.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([os, count]) => ({ os, count, hourlyUsd: round2(count * hourlyRate(os, draft.size)) }));

  return {
    computers: members.length,
    maxHourlyUsd: round2(breakdown.reduce((sum, b) => sum + b.hourlyUsd, 0)),
    breakdown,
  };
}

export interface CreateFleetResult {
  fleet: Fleet;
  spaces: Space[];
}

/**
 * Simulate fleet creation locally. Pure: given the same inputs it yields the
 * same ids and names. New Spaces start in `provisioning` and are stamped with
 * `now` so they float to the top of the MRU order.
 *
 * This never contacts a backend. The caller is responsible for merging the
 * returned Spaces into UI state.
 */
export function createFleetLocally(
  draft: FleetDraft,
  existing: readonly Space[],
  now: number,
): CreateFleetResult {
  const template = getTemplate(draft.templateId);
  const members = resolveMembers(draft);
  const ordinal = existing.filter((s) => s.fleetId).reduce((set, s) => set.add(s.fleetId!), new Set<string>()).size + 1;
  const fleetId = `fleet-${ordinal}-${template.id}`;
  const usedNames = new Set(existing.map((s) => s.name));

  const spaces: Space[] = members.map((m, index) => {
    const name = uniqueName(m.name, usedNames);
    usedNames.add(name);
    return {
      id: `${fleetId}-${index + 1}`,
      name,
      os: m.os,
      status: "provisioning",
      detail: "Starting",
      // Slightly staggered so MRU order within the fleet matches member order.
      lastUsedAt: now - index,
      scene: m.scene,
      fleetId,
      size: draft.size,
      region: draft.region,
    };
  });

  const fleet: Fleet = {
    id: fleetId,
    name: template.id === "custom" ? `Custom group ${ordinal}` : template.name,
    templateId: template.id,
    createdAt: now,
    spaceIds: spaces.map((s) => s.id),
    region: draft.region,
    size: draft.size,
    autoSuspend: draft.autoSuspend,
  };

  return { fleet, spaces };
}

function uniqueName(base: string, used: ReadonlySet<string>): string {
  if (!used.has(base)) return base;
  let n = 2;
  while (used.has(`${base} ${n}`)) n += 1;
  return `${base} ${n}`;
}

function round2(n: number): number {
  return Math.round(n * 100) / 100;
}

export function formatUsd(n: number): string {
  return `$${n.toFixed(2)}`;
}
