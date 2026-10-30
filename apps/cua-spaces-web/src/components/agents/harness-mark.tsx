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

/** A provider's one-colour mark, drawn once in dark ink and once in light ink (public/onboarding/marks/providers). */
const providerMark = (name: string): AppMark => ({ light: `${M}providers/${name}.svg`, dark: `${M}providers/${name}-dark.svg`, scale: 0.9 });

/** The providers with a mark (keyed `provider:<id>`, as the Providers list asks for them). The rest get the letter tile. */
const PROVIDER_MARKS: Record<string, AppMark> = {
  "provider:openrouter": providerMark("openrouter"),
  "provider:xai": providerMark("xai"),
  "provider:xai-oauth": providerMark("xai"),
  "provider:deepseek": providerMark("deepseek"),
  "provider:kimi": providerMark("kimi"),
  "provider:alibaba": providerMark("alibabacloud"),
  "provider:qwen-oauth": providerMark("qwen"),
  "provider:ai-gateway": providerMark("vercel"),
  "provider:arcee": providerMark("arcee"),
  "provider:azure-foundry": providerMark("azureai"),
  "provider:bedrock": providerMark("bedrock"),
  "provider:commandcode": providerMark("commandcode"),
  "provider:deepinfra": providerMark("deepinfra"),
  "provider:fireworks": providerMark("fireworks"),
  "provider:gmi": providerMark("gmicloud"),
  "provider:huggingface": providerMark("huggingface"),
  "provider:kilocode": providerMark("kilocode"),
  "provider:meta-ai": providerMark("metaai"),
  "provider:minimax": providerMark("minimax"),
  "provider:minimax-oauth": providerMark("minimax"),
  "provider:nebius-token-factory": providerMark("nebius"),
  "provider:nous": providerMark("nousresearch"),
  "provider:novita": providerMark("novita"),
  "provider:nvidia": providerMark("nvidia"),
  "provider:ollama-cloud": providerMark("ollama"),
  "provider:opencode-zen": providerMark("opencode"),
  "provider:opencode-go": providerMark("opencode"),
  "provider:stepfun": providerMark("stepfun"),
  "provider:tencent-tokenhub": providerMark("tencent"),
  "provider:upstage": providerMark("upstage"),
  "provider:vertex": providerMark("vertexai"),
  "provider:xiaomi": providerMark("xiaomimimo"),
  "provider:zai": providerMark("zai"),
};

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
  t3code: both("mark-t3code.png"),
  ...PROVIDER_MARKS,
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
export function HarnessMark({ agent, name, large, plain }: { agent: string; name: string; large?: boolean; plain?: boolean }) {
  const has = agent in APP_MARKS;
  // In a header: just the logo, no tile (a tile reads as something to pick).
  if (plain) return has ? <AppMarkImage agent={agent} className="size-5 shrink-0" /> : null;
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
