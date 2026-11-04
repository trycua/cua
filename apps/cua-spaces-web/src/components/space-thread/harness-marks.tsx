// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The harnesses an agent member can run, in the app's order (Hermes,
 * Claude Code, Codex, Cursor, then OpenClaw and T3 Code), with the official
 * mark on an agent's tile: the coding-app list's official marks (`APP_MARKS`,
 * sources and terms in public/onboarding/marks/NOTICE). A harness the app doesn't
 * know gets the bot glyph on the agent's own colour.
 */

import { BotIcon } from "lucide-react";

import { APP_MARKS } from "@/components/agents/harness-mark";
import { cn } from "@/lib/utils";

import { agentHue } from "./avatar";

export interface HarnessInfo {
  id: string;
  /** The product's name, as its maker writes it. */
  name: string;
  maker: string;
  /** The mark's files under `public/` (dark: a variant for the dark theme), or null for the bot glyph. */
  mark: { src: string; dark?: string; bg: string; pad?: number } | null;
  /** Other ids daemons use for it. */
  aliases?: readonly string[];
}

/**
 * The official mark for a harness the coding-app list has (`APP_MARKS`, one
 * map for onboarding, Connectors, the Agents page and the thread; sources
 * in public/onboarding/marks/NOTICE), on a quiet tile.
 */
const official = (id: string): HarnessInfo["mark"] => {
  const m = APP_MARKS[id];
  return m ? { src: m.light, ...(m.dark !== m.light ? { dark: m.dark } : {}), bg: "transparent", pad: 0.18 } : null;
};

/** Every harness the app names, in order, each with its official mark. */
export const HARNESSES: readonly HarnessInfo[] = [
  { id: "hermes", name: "Hermes Agent", maker: "Nous Research", mark: official("hermes") },
  { id: "claude-code", name: "Claude Code", maker: "Anthropic", mark: official("claude-code"), aliases: ["claude"] },
  { id: "codex", name: "Codex CLI", maker: "OpenAI", mark: official("codex"), aliases: ["openai-codex"] },
  { id: "cursor", name: "Cursor", maker: "Anysphere", mark: official("cursor"), aliases: ["cursor-agent", "cursor-cli"] },
  { id: "openclaw", name: "OpenClaw", maker: "Open source", mark: official("openclaw") },
  { id: "t3code", name: "T3 Code", maker: "T3 Tools", mark: official("t3code"), aliases: ["t3", "t3-code"] },
];


const BY_ID = new Map<string, HarnessInfo>(HARNESSES.flatMap((h) => [[h.id, h] as const, ...(h.aliases ?? []).map((a) => [a, h] as const)]));

/** The harness for an id (or one of its aliases), or null for one the app doesn't know. */
export function harnessInfo(id: string | null | undefined): HarnessInfo | null {
  return id ? (BY_ID.get(id.toLowerCase()) ?? null) : null;
}

/** The harness's name for an id, or null when unknown. */
export const harnessLabel = (id: string | null | undefined): string | null => harnessInfo(id)?.name ?? null;

/**
 * A harness's mark filling a square tile of `px`: the official mark, an
 * https icon the agent set (`icon`), or the bot glyph on `hue` when there is
 * neither. The caller clips the corners.
 */
export function HarnessMark({ harness, icon, px, hue, className }: { harness: string; icon?: string | null; px: number; hue?: number; className?: string }) {
  const custom = icon && icon !== "harness" && /^https:\/\//i.test(icon) ? icon : null;
  if (custom) {
    return (
      <span data-harness-mark="custom" className={cn("flex items-center justify-center overflow-hidden bg-muted", className)} style={{ width: px, height: px }}>
        <img src={custom} alt="" referrerPolicy="no-referrer" draggable={false} className="size-full object-cover" />
      </span>
    );
  }
  const info = harnessInfo(harness);
  const m = info?.mark;
  if (!m) {
    return (
      <span
        data-harness-mark="bot"
        className={cn("flex items-center justify-center text-white", className)}
        style={{ width: px, height: px, backgroundColor: `oklch(0.62 0.14 ${hue ?? agentHue(harness, harness)})` }}
      >
        <BotIcon style={{ width: px * 0.55, height: px * 0.55 }} strokeWidth={2} />
      </span>
    );
  }
  const pad = m.pad ? Math.round(px * m.pad) : 0;
  const img = (src: string, cls?: string) => <img src={src} alt="" draggable={false} className={cn("size-full object-contain", cls)} />;
  return (
    <span
      data-harness-mark={info.id}
      className={cn("flex items-center justify-center overflow-hidden", m.bg === "transparent" && "bg-foreground/[0.06]", className)}
      style={{ width: px, height: px, background: m.bg === "transparent" ? undefined : m.bg, padding: pad }}
    >
      {m.dark ? (
        <>
          {img(m.src, "dark:hidden")}
          {img(m.dark, "hidden dark:block")}
        </>
      ) : (
        img(m.src)
      )}
    </span>
  );
}
