// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The repo chip's data: what a pasted link or `owner/name` means, and the
 * repos used lately (kept on this computer). Only the shape is checked here;
 * whether the repo can be read is the host's to say once a GitHub
 * connection holds the access (a private repo cannot be probed without it).
 */

import { useSyncExternalStore } from "react";

/** A repo a task starts from. */
export interface AskRepo {
  /** The clone URL, always https. */
  url: string;
  /** What the chip shows: `owner/name`. */
  label: string;
}

const NAME = "[A-Za-z0-9_.-]+";

/**
 * A repo from what was typed: `owner/name`, `github.com/owner/name`, an
 * https link to a repo or to one of its pages (an issue, a pull request, a
 * folder), or an ssh link. Null for anything else.
 */
export function parseRepo(input: string): AskRepo | null {
  const t = input.trim();
  if (!t || /\s/.test(t)) return null;
  const short = new RegExp(`^(${NAME})/(${NAME})$`).exec(t);
  if (short && !t.includes("..")) return repo("github.com", short[1]!, short[2]!);
  const ssh = new RegExp(`^git@([A-Za-z0-9.-]+):(${NAME})/(${NAME})$`).exec(t);
  if (ssh) return repo(ssh[1]!, ssh[2]!, ssh[3]!);
  const web = new RegExp(`^(?:https?://)?([A-Za-z0-9.-]+\\.[A-Za-z]{2,})/(${NAME})/(${NAME})(?:[/?#].*)?$`).exec(t);
  if (web) return repo(web[1]!.toLowerCase(), web[2]!, web[3]!);
  return null;
}

function repo(host: string, owner: string, name: string): AskRepo | null {
  const clean = name.replace(/\.git$/, "");
  if (!owner || !clean || owner === "." || clean === "." || clean.startsWith("..")) return null;
  return { url: `https://${host}/${owner}/${clean}`, label: `${owner}/${clean}` };
}

/* ---- the repo of the next task, and the ones used lately ---- */

const KEY = "cua.askFirst.repos";
const RECENT = 5;

interface Saved {
  /** The repo the next task starts from (kept until removed). */
  current: AskRepo | null;
  recent: AskRepo[];
}

let cache: Saved | undefined;
const listeners = new Set<() => void>();

function load(): Saved {
  if (cache) return cache;
  try {
    const raw = globalThis.localStorage?.getItem(KEY);
    const v = raw ? (JSON.parse(raw) as Partial<Saved>) : {};
    const ok = (r: unknown): r is AskRepo => !!r && typeof (r as AskRepo).url === "string" && typeof (r as AskRepo).label === "string";
    cache = { current: ok(v.current) ? v.current : null, recent: (Array.isArray(v.recent) ? v.recent : []).filter(ok).slice(0, RECENT) };
  } catch {
    cache = { current: null, recent: [] };
  }
  return cache;
}

function save(next: Saved): void {
  cache = next;
  try {
    globalThis.localStorage?.setItem(KEY, JSON.stringify(next));
  } catch {
    // Kept for this session only.
  }
  for (const l of [...listeners]) l();
}

/** Picks `r` for the next task and puts it first among the recent ones. */
export function chooseRepo(r: AskRepo): void {
  const s = load();
  save({ current: r, recent: [r, ...s.recent.filter((x) => x.url !== r.url)].slice(0, RECENT) });
}

/** Takes the repo off the next task (it stays among the recent ones). */
export function clearRepo(): void {
  save({ ...load(), current: null });
}

/** For tests: forget everything kept. */
export function resetRepos(): void {
  cache = undefined;
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

const snapshot = () => load();
const empty: Saved = { current: null, recent: [] };

/** The repo of the next task and the recent ones (re-renders when they change). */
export function useRepos(): Saved {
  return useSyncExternalStore(subscribe, snapshot, () => empty);
}
