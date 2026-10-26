// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cn } from "@/lib/utils";

/**
 * A coding app's official mark, one file per app theme. `light` and `dark` are
 * the maker's own variants (the same file when its full-colour mark is made for
 * both). `scale` evens out optical size: marks shipped with built-in clear space
 * grow, marks drawn as a filled square shrink. Contrast against the row in each
 * theme is at least 3:1 (Claude's Clay spark in light mode, 2.9:1, is the
 * only colour Anthropic issues it in).
 */
export type AppMark = { light: string; dark: string; scale?: number };

const M = "/onboarding/marks/";
const both = (file: string, scale?: number): AppMark => ({ light: M + file, dark: M + file, scale });
const pair = (id: string, scale?: number): AppMark => ({ light: `${M}mark-${id}.svg`, dark: `${M}mark-${id}-dark.svg`, scale });

/**
 * Every coding app with a usable official mark. Sources and brand guidelines
 * are in public/onboarding/marks/NOTICE. Apps not listed (Kiro) get the letter tile.
 */
export const APP_MARKS: Record<string, AppMark> = {
  "claude-code": both("mark-claude.svg"),
  codex: pair("openai", 1.8),
  "openai-codex": pair("openai", 1.8),
  hermes: pair("hermes"),
  openclaw: both("mark-openclaw.svg"),
  cursor: pair("cursor"),
  vscode: both("mark-vscode.svg"),
  "gemini-cli": { light: `${M}mark-gemini-cli.svg`, dark: `${M}mark-gemini-cli-dark.svg`, scale: 0.85 },
  cline: pair("cline"),
  opencode: pair("opencode", 0.85),
  pi: pair("pi", 0.8),
  windsurf: pair("windsurf"),
  "copilot-cli": pair("copilot"),
  amp: both("mark-amp.svg", 0.85),
  goose: pair("goose"),
  zed: pair("zed", 0.85),
  antigravity: both("mark-antigravity.svg", 1.15),
};

/** The app's mark at the size `className` gives, switched with the app theme; null when it has none. */
export function AppMarkImage({ agent, className }: { agent: string; className?: string }) {
  const mark = APP_MARKS[agent];
  if (!mark) return null;
  const style = mark.scale ? { transform: `scale(${mark.scale})` } : undefined;
  const img = (src: string, theme: "light" | "dark" | "both") => (
    <img
      src={src}
      alt=""
      aria-hidden
      draggable={false}
      style={style}
      data-app-mark={agent}
      data-theme={theme}
      className={cn("shrink-0 object-contain", className, theme === "light" && "dark:hidden", theme === "dark" && "hidden dark:block")}
    />
  );
  if (mark.light === mark.dark) return img(mark.light, "both");
  return (
    <>
      {img(mark.light, "light")}
      {img(mark.dark, "dark")}
    </>
  );
}

/** A coding app's logo in a small tile; a neutral initial tile when there is no logo. */
export function HarnessMark({ agent, name, large }: { agent: string; name: string; large?: boolean }) {
  const has = agent in APP_MARKS;
  return (
    <span
      className={cn(
        "flex shrink-0 items-center justify-center rounded-lg border bg-card font-semibold text-muted-foreground dark:bg-background",
        large ? "size-10 text-[14px]" : "size-8 text-xs",
      )}
      data-harness-mark={agent}
      aria-hidden
    >
      {has ? <AppMarkImage agent={agent} className={large ? "size-6" : "size-5"} /> : name.trim().charAt(0).toUpperCase() || "?"}
    </span>
  );
}
