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

/** "High", "Extra high": a level as the person reads it. */
export function effortLabel(level: string): string {
  if (level === "xhigh") return "Extra high";
  return level.charAt(0).toUpperCase() + level.slice(1);
}

/** A level as a segment of the menu's row says it: "Low", "Med", "High", "XHigh", "Max". */
export function effortShort(level: string): string {
  if (level === "medium") return "Med";
  if (level === "xhigh") return "XHigh";
  return effortLabel(level);
}

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
export function chipWords(harnessLabel: string, modelLabel: string, effort: string | null): { lead: string; trail: string } {
  const model = shortModelName(modelLabel);
  return effort ? { lead: model, trail: effortLabel(effort) } : { lead: harnessLabel, trail: model };
}
