// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Pure view logic, kept out of the components so it is testable without a
// webview.

import { requiresCuaApp } from "@trycua/cua/teleport"

/** A transcript line from the core: what the user typed, an agent message,
 *  or an activity group (the SDK's fold; `text` is its summary). */
export interface Line {
  turn: number
  speaker: "user" | "agent" | "activity"
  text: string
  /** Activity only: one line per step. */
  steps?: string[]
}

export interface TeleportItem {
  relative_path: string
  label: string
  estimated_bytes: number
  is_sensitive: boolean
  is_checked_by_default: boolean
}

export interface TeleportManifest {
  app: string
  display_name: string
  items: TeleportItem[]
  total_estimated_bytes: number
  notes: string[]
}

export interface Decision {
  include: string[] | null
  acknowledge_sensitive: boolean
}

/** `host:port`, a URL or a Space id, as `spaces.add` takes it; `null` when unusable. */
export function normalizeSpaceUrl(input: string): string | null {
  const s = input.trim()
  if (!s) return null
  // Space ids (sandbox refs, and the legacy space:// spelling) pass through.
  if (/^(space:\/\/|(local|cloud|direct|relay):)/.test(s)) return s
  const withScheme = /^[a-z]+:\/\//i.test(s) ? s : `http://${s}`
  try {
    const u = new URL(withScheme)
    if (!["http:", "https:"].includes(u.protocol) || !u.hostname) return null
    if (!u.port && !/^[a-z]+:\/\//i.test(s)) return null // bare host needs a port
    return withScheme.replace(/\/$/, "")
  } catch {
    return null
  }
}

/** Two-letter avatar initials. */
export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean)
  if (parts.length === 0) return "?"
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase()
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase()
}

/** Transcript lines grouped by turn, in order. */
export function groupByTurn(lines: Line[]): { turn: number; lines: Line[] }[] {
  const out: { turn: number; lines: Line[] }[] = []
  for (const l of lines) {
    const last = out[out.length - 1]
    if (last && last.turn === l.turn) last.lines.push(l)
    else out.push({ turn: l.turn, lines: [l] })
  }
  return out
}

/**
 * The approval dialog's answer. Selecting exactly the defaults sends
 * `include: null` (the provider's default set); sensitive items need the
 * explicit acknowledgement, otherwise there is no decision to send.
 */
export function decide(
  manifest: TeleportManifest,
  selected: Set<string>,
  acknowledged: boolean,
): { decision: Decision | null; reason: string } {
  if (selected.size === 0) return { decision: null, reason: "Select at least one item." }
  const sensitive = manifest.items.some((i) => i.is_sensitive && selected.has(i.relative_path))
  if (sensitive && !acknowledged) {
    return { decision: null, reason: "Acknowledge the sensitive items (cookies, logins) first." }
  }
  const defaults = manifest.items.filter((i) => i.is_checked_by_default).map((i) => i.relative_path)
  const isDefault = defaults.length === selected.size && defaults.every((p) => selected.has(p))
  return {
    decision: { include: isDefault ? null : [...selected].sort(), acknowledge_sensitive: acknowledged },
    reason: "",
  }
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}

/** What the app says where it used to offer app teleport ("Teleport an
 *  app...", dropped app bundles). */
export const APP_TELEPORT_IN_CUA_SPACES = "App teleport ships with Cua Spaces (source-available)."

/** What the app says when session teleport is refused because this runtime
 *  has no Cua Spaces teleport. */
export const SESSION_TELEPORT_IN_CUA_SPACES = "Session teleport ships with Cua Spaces (source-available)."

/**
 * A person-facing message for a core error, with the raw text kept as the
 * detail. The spacesd answers a wrong or missing token with gRPC
 * `unauthenticated`; say that in words.
 */
export function friendlyError(raw: string): {
  message: string
  detail: string
  /** A button to show with the message (the Install Cua affordance). */
  link?: { label: string; url: string }
} {
  const text = raw.replace(/^Error:\s*/, "")
  // The Keyvault refuses session teleport from an embedded SDK by design;
  // say what to do instead of showing the refusal code.
  const install = requiresCuaApp(text)
  if (install)
    return { message: `${install.title}. ${install.message}`, detail: text, link: { label: install.actionLabel, url: install.url } }
  // The in-process runtime has no teleport: it ships with Cua Spaces.
  if (/^teleport is not available on this host: .*ships with Cua Spaces/i.test(text))
    return { message: SESSION_TELEPORT_IN_CUA_SPACES, detail: text }
  if (/unauthenticated|invalid bearer|missing or invalid .*token|401/i.test(text))
    return { message: "The Space rejected the token. Check the token and try again.", detail: text }
  if (/connection refused|failed to connect|dns error|timed out/i.test(text))
    return { message: "Could not reach the Space. Check the address and that it is running.", detail: text }
  return { message: text, detail: "" }
}
