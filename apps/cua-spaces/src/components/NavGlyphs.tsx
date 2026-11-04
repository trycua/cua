// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import claudeMark from "../assets/agents/claude-code.svg";
import codexMark from "../assets/agents/openai-codex.svg";

import { SfIcon } from "./SfIcon";

/**
 * The small chrome glyphs the switcher grew with the per-window view: the back
 * caret next to a Space's name, the list-view toggle beside the gear, and the
 * two per-Space actions (picture-in-picture, whole-desktop stream) that take
 * the gear's place while a Space is open. Each prefers the real SF Symbol and
 * falls back to an inline stroke glyph, exactly like `GearGlyph`.
 */
export function BackGlyph({ className }: { className?: string }) {
  return (
    <SfIcon
      name="chevron.backward"
      className={className}
      fallback={
        <svg className={className} viewBox="0 0 24 24" width="18" height="18" aria-hidden="true" fill="none">
          <path d="M15 4.5 7.5 12 15 19.5" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      }
    />
  );
}

export function ListGlyph({ className }: { className?: string }) {
  return (
    <SfIcon
      name="list.bullet"
      className={className}
      fallback={
        <svg className={className} viewBox="0 0 24 24" width="18" height="18" aria-hidden="true" fill="none">
          <g stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
            <path d="M9 6.5h11M9 12h11M9 17.5h11" />
            <path d="M4.5 6.5h.01M4.5 12h.01M4.5 17.5h.01" strokeWidth="2.6" />
          </g>
        </svg>
      }
    />
  );
}

export function PipGlyph({ className }: { className?: string }) {
  return (
    <SfIcon
      name="pip.enter"
      className={className}
      fallback={
        <svg className={className} viewBox="0 0 24 24" width="18" height="18" aria-hidden="true" fill="none">
          <rect x="2.75" y="4.75" width="18.5" height="14.5" rx="2.5" stroke="currentColor" strokeWidth="1.6" />
          <rect x="12" y="12" width="7.5" height="5.75" rx="1.4" fill="currentColor" />
        </svg>
      }
    />
  );
}

export function DesktopGlyph({ className }: { className?: string }) {
  return (
    <SfIcon
      name="display"
      className={className}
      fallback={
        <svg className={className} viewBox="0 0 24 24" width="18" height="18" aria-hidden="true" fill="none">
          <rect x="2.75" y="4" width="18.5" height="13" rx="2.2" stroke="currentColor" strokeWidth="1.6" />
          <path d="M9 20h6" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
        </svg>
      }
    />
  );
}

/**
 * What a Space runs, as a NEUTRAL platform glyph — a laptop, a desktop tower,
 * a terminal — not a vendor logo. The app ships no Apple/Windows/Linux brand
 * marks and this is not the place to start: the licensing around those marks
 * is a real constraint a generic symbol simply does not have. On macOS these
 * resolve to Apple's own SF Symbols (`laptopcomputer`, `pc`, `terminal`), with
 * inline fallbacks everywhere else.
 *
 * Returns `null` for an OS the provider did not report, so the caller can fall
 * back to its monogram rather than show an empty slot.
 */
export function OsGlyph({ os, className }: { os?: string; className?: string }) {
  switch (os) {
    case "macos":
      return (
        <SfIcon
          name="laptopcomputer"
          className={className}
          fallback={
            <svg className={className} viewBox="0 0 24 24" width="18" height="18" aria-hidden="true" fill="none">
              <rect x="4" y="5" width="16" height="10" rx="1.6" stroke="currentColor" strokeWidth="1.6" />
              <path d="M2.5 18.5h19" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
            </svg>
          }
        />
      );
    case "windows":
      return (
        <SfIcon
          name="pc"
          className={className}
          fallback={
            <svg className={className} viewBox="0 0 24 24" width="18" height="18" aria-hidden="true" fill="none">
              <rect x="6" y="3.5" width="12" height="17" rx="1.8" stroke="currentColor" strokeWidth="1.6" />
              <path d="M9 7h6M9 10.5h6" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
              <circle cx="12" cy="16.5" r="1.1" fill="currentColor" />
            </svg>
          }
        />
      );
    case "linux":
      return (
        <SfIcon
          name="terminal"
          className={className}
          fallback={
            <svg className={className} viewBox="0 0 24 24" width="18" height="18" aria-hidden="true" fill="none">
              <rect x="3" y="4.5" width="18" height="15" rx="2" stroke="currentColor" strokeWidth="1.6" />
              <path d="m7.5 9.5 3 2.5-3 2.5M12.5 15h4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          }
        />
      );
    default:
      return null;
  }
}

/** Human label for a Space's OS, for the glyph's accessible name. */
export function osLabel(os?: string): string | null {
  switch (os) {
    case "macos":
      return "macOS";
    case "windows":
      return "Windows";
    case "linux":
      return "Linux";
    default:
      return null;
  }
}

/**
 * The real mark of an agent harness, for the AGENTS rows.
 *
 * This is the same job an app's icon does in the window rows above: say which
 * harness a run belongs to at a glance. Both files in ../assets/agents are the
 * official marks (see the README there) — neither CLI is an `.app` bundle, so
 * there is no guest icon to ask `NSWorkspace` for the way `space_app_icon` does.
 *
 * A harness with no mark renders NOTHING rather than a generic glyph. A shared
 * fallback drawn in every unrecognised row does not identify anything; it just
 * makes two different harnesses look like the same one.
 */
const AGENT_MARKS: Record<string, string> = {
  "claude-code": claudeMark,
  "openai-codex": codexMark,
};

export function agentMark(agent: string): string | undefined {
  return AGENT_MARKS[agent];
}

export function AgentGlyph({ agent, className }: { agent: string; className?: string }) {
  const mark = agentMark(agent);
  if (!mark) return null;
  return <img className={className} src={mark} alt="" aria-hidden="true" draggable={false} />;
}
