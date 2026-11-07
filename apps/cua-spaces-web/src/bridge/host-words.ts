// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The app core words the computer it runs on as "this Mac" wherever it has no
 * system to go by. On a PC or a Linux machine the page says what the host
 * actually is ("This PC", "This computer") at the one place the core's answers
 * enter the page, from the system the shell reports ({@link hostOs}).
 *
 * Display only: nothing is written back, so a stored name keeps its text and
 * shows in its host's words (a Windows user's old "This Mac" reads "This PC";
 * a Mac user's names are untouched). Text that is about macOS itself (Lume,
 * Keychain, Finder, ...) is left alone, and so are "Mac mini" and other
 * people's Macs: only the phrases that mean this computer are replaced.
 */

import { hostOs, THIS_MACHINE, type HostOs } from "./host-os";

/** Copy that is about macOS itself: left as the core wrote it. */
const MAC_ONLY = /macOS|Lume|Keychain|Finder|Xcode|Intel|Touch ID|Apple/;

const LOWER: Record<HostOs, string> = { macos: "this Mac", windows: "this PC", linux: "this computer" };
const YOUR: Record<HostOs, string> = { macos: "your Mac", windows: "your PC", linux: "your computer" };

/** One string in the words of `os`. */
export function hostWords(text: string, os: HostOs = hostOs()): string {
  if (os === "macos" || !/\b(?:this|your) Mac\b/i.test(text) || MAC_ONLY.test(text)) return text;
  return text
    .replace(/\bThis Mac\b/g, THIS_MACHINE[os])
    .replace(/\bthis Mac\b/g, LOWER[os])
    .replace(/\bYour Mac\b/g, capital(YOUR[os]))
    .replace(/\byour Mac\b/g, YOUR[os]);
}

const capital = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** Every string of a core answer (keys are never touched) in the words of `os`. */
export function localizeHostWords<T>(value: T, os: HostOs = hostOs()): T {
  if (os === "macos") return value;
  return walk(value, os) as T;
}

function walk(value: unknown, os: HostOs): unknown {
  if (typeof value === "string") return hostWords(value, os);
  if (Array.isArray(value)) return value.map((v) => walk(v, os));
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value)) out[k] = walk(v, os);
    return out;
  }
  return value;
}
