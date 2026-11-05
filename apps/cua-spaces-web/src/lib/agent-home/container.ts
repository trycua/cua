// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { AbilityDescriptor, AgentHomeRuntime, HomeCreate, HomeSetup } from "./abilities";

/**
 * The agent home runs on the slim tier of the Linux image: the catalog entry
 * `ghcr.io/trycua/linux:24.04-slim` (libs/images/sandbox-images.json). It needs
 * the `cua` CLI and a spawn-capable cua-spacesd in that image, which the image
 * recipe provides. A test keeps this constant equal to that entry.
 */
export const AGENT_HOME_IMAGE = "ghcr.io/trycua/linux:24.04-slim";

/** The labels the daemon reads (`children/home.rs`); a test keeps the two in step. */
export const HOME_LABELS = {
  home: "cua.home",
  agent: "cua.home.agent",
  list: "cua.home.list",
  mentions: "cua.home.mentions",
  machines: "cua.home.machines",
} as const;

/** The container home's abilities, as `cua` commands the agent runs inside it. */
const ABILITIES: AbilityDescriptor[] = [
  {
    id: "spaces.list",
    call: "cua spaces ls",
    summary: "See Spaces. You see this tree and the Spaces the person @mentioned. Seeing every Space needs their OK.",
    asksFirst: true,
  },
  {
    id: "spaces.spawn",
    call: "cua spaces create <linux|windows|macos> --name <name> [--image <image>]",
    summary: "Make a child Space of the OS and image you need. It must fit this tree's budget and it can only have less network than you do.",
    asksFirst: false,
  },
  {
    id: "machines.use",
    call: "cua spaces create <os> --on host:<computer>",
    summary: "Put a child on one of the person's computers. The person must let you use that computer first, unless they named it.",
    asksFirst: true,
  },
];

/** A label value holds up to 256 bytes (the daemon's limit): keep the first words that fit. */
export function joinLabel(xs: readonly string[], max = 256): string {
  let out = "";
  for (const x of [...new Set(xs.filter(Boolean))].sort()) {
    const next = out ? `${out},${x}` : x;
    if (new TextEncoder().encode(next).length > max) break;
    out = next;
  }
  return out;
}
const join = (xs: readonly string[]) => joinLabel(xs);

export const containerHome: AgentHomeRuntime = {
  kind: "container",
  abilities: () => ABILITIES.map((a) => ({ ...a })),
  create(setup: HomeSetup, name: string): HomeCreate {
    const labels: Record<string, string> = { [HOME_LABELS.home]: "1", [HOME_LABELS.agent]: setup.agent };
    if (setup.grants.listAll) labels[HOME_LABELS.list] = "all";
    const mentions = [...setup.mentions.spaces.flatMap((s) => [s.id, s.name]), ...setup.mentions.machines.flatMap((m) => [m.id, m.name])];
    if (mentions.length) labels[HOME_LABELS.mentions] = join(mentions);
    // A machine the person granted before, or named now, may take children.
    const machines = [...setup.grants.machines, ...setup.mentions.machines.flatMap((m) => [m.id, m.name])];
    if (machines.length) labels[HOME_LABELS.machines] = join(machines);
    return { image: AGENT_HOME_IMAGE, on: "local", kind: "container", name, cpus: 1, memoryMb: 1024, spacesd: true, children: true, labels };
  },
};
