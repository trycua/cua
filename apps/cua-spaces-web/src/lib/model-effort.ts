// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Reasoning levels (effort) of the models the agent menu lists, and the words
 * of the agent chip. The chip does not repeat the app (its logo is beside
 * it): a reasoning model reads "Sonnet 5.5  High" (the model, then the level
 * quietly) and any other reads "OpenCode  Kimi K3" (the app, then the model
 * quietly). The two parts are laid out with spacing, never a dot or a pipe.
 */

export interface EffortLevels {
  /** The levels in order, lowest first, as the ids the app takes. */
  levels: string[];
  /** The level a run uses when none was chosen. */
  default: string;
}

const CLAUDE: EffortLevels = { levels: ["low", "medium", "high", "xhigh", "max"], default: "high" };
const CODEX: EffortLevels = { levels: ["low", "medium", "high", "xhigh"], default: "medium" };

/**
 * What the model offers, for the app that runs it; null for a model without
 * levels (a quick model, an older id, a model the app picks its own way).
 */
export function effortLevels(harness: string, modelId: string): EffortLevels | null {
  const id = modelId.toLowerCase();
  if (harness === "claude-code") {
    // Haiku is the quick tier and the 4.x ids are kept only so a saved pick still runs.
    if (id.includes("haiku") || /claude-(sonnet|opus)-[0-4](-|$)/.test(id) || id === "claude-sonnet-5") return null;
    return /claude-(sonnet|opus|fable)-/.test(id) ? CLAUDE : null;
  }
  if (harness === "openai-codex") return /^gpt-5/.test(id) ? CODEX : null;
  return null;
}

/** The catalog key of a level's name; `short` for the segment of the menu's row ("Med"), else the chip's ("Medium"). */
export const effortKey = (level: string, short = false) => `home.agent_picker.effort_${short ? "short_" : ""}${level}`;

/** The model as the chip names it: "Sonnet 5.5", not "Claude Sonnet 5.5". */
export function shortModelName(label: string): string {
  return label.replace(/^Claude\s+/i, "").trim();
}

/** The level in use: the chosen one while the model offers it, else the model's own default, else none. */
export function effortOf(harness: string, modelId: string, chosen?: string | null): string | null {
  const e = effortLevels(harness, modelId);
  if (!e) return null;
  return chosen && e.levels.includes(chosen) ? chosen : e.default;
}

/** The chip's words: `lead` in the normal weight, `trail` quiet after it. */
export function chipWords(harnessLabel: string, modelLabel: string, effort: string | null, say: (key: string) => string): { lead: string; trail: string } {
  const model = shortModelName(modelLabel);
  return effort ? { lead: model, trail: say(effortKey(effort)) } : { lead: harnessLabel, trail: model };
}
