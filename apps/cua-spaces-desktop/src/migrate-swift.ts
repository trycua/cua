// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// macOS: the first launch of this app takes over the Swift app's state
// (apps/cua-spaces-macos), once. The two share the bundle id
// com.trycua.spaces.macos, so a Sparkle update or a beta install replaces
// the Swift app in place (docs/sparkle-cutover.md); this keeps what the user
// set there. Nothing of the Swift app's is changed or removed, so going back
// to it loses nothing.
//
// What the Swift app keeps, and where it goes:
// - ~/Library/Application Support/com.trycua.spaces.macos/settings.json, the
//   app core's settings (hotkey, switcher, default location, update channel,
//   experiments, Keyvault and teleport choices, ...): copied as is to
//   app-settings.json in userData (same Rust core, same schema; the bridge
//   loads it from appCoreStatePaths). Its update channel also becomes this
//   app's `updateChannel`.
// - .../onboarding.json, the first run's progress: copied as is.
// - The defaults domain com.trycua.spaces.macos: the window frame
//   ("NSWindow Frame CuaSpacesWebUI") becomes `windowBounds`; Sparkle's
//   SUEnableAutomaticChecks / SUAutomaticallyUpdate are recorded in the
//   marker (the cutover reads them) but turn nothing on. The rest is the
//   Swift window's colour cache (WebUIBackground*), its native video switch
//   (WebUINativeVideo) and Sparkle's bookkeeping, none of which applies here.
// - ~/.cua is shared by both apps and needs nothing.
// Not carried over: the web UI's WebKit localStorage (the theme choice;
// onboarding's authoritative state is onboarding.json).
//
// The marker is `swiftMigration` in settings.json: present, nothing runs.
import { copyFileSync, existsSync, mkdirSync, readFileSync } from "node:fs";
import * as path from "node:path";
import type { Settings, SwiftMigration, WindowBounds } from "./settings";

export const SWIFT_BUNDLE_ID = "com.trycua.spaces.macos";
export const SWIFT_FRAME_KEY = "NSWindow Frame CuaSpacesWebUI";

/** Where this app keeps the app core's files (the Swift app's support directory's equivalents). */
export const appCoreStatePaths = (userData: string) => ({
  settings: path.join(userData, "app-settings.json"),
  onboarding: path.join(userData, "onboarding.json"),
});

export const swiftSupportDir = (home: string) => path.join(home, "Library/Application Support", SWIFT_BUNDLE_ID);

type PlistValue = string | number | boolean | PlistValue[] | { [key: string]: PlistValue };

const ENTITIES: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'" };
const decode = (s: string) =>
  s.replace(/&(#x[0-9a-f]+|#\d+|amp|lt|gt|quot|apos);/gi, (_, e: string) =>
    e[0] === "#" ? String.fromCodePoint(Number(e[1] === "x" || e[1] === "X" ? `0${e.slice(1)}` : e.slice(1))) : (ENTITIES[e] ?? ""),
  );

/** An XML property list (what `defaults export <domain> -` prints). Dates and data stay strings. */
export function parsePlist(xml: string): PlistValue {
  const tokens = [...xml.replace(/<\?xml[^>]*\?>|<!DOCTYPE[^>]*>|<!--[\s\S]*?-->/g, "").matchAll(/<(\/?)([a-z]+)(?:\s[^>]*?)?(\/?)>|([^<]+)/gi)];
  let i = 0;
  /** The text at token j, undefined for a tag or past the end. */
  const txt = (j: number): string | undefined => tokens[j]?.[4];
  const skipText = () => {
    while (!(txt(i) ?? "x").trim()) i++;
  };
  const text = (tag: string): string => {
    let out = "";
    for (let t = txt(i); t !== undefined; t = txt(++i)) out += t;
    const close = tokens[i++];
    if (!close || close[1] !== "/" || close[2] !== tag) throw new Error(`plist: unclosed <${tag}>`);
    return decode(out);
  };
  const value = (): PlistValue => {
    skipText();
    const t = tokens[i++];
    if (!t || t[4] !== undefined || t[1] === "/") throw new Error("plist: expected a value");
    const tag = t[2];
    if (t[3] === "/") {
      if (tag === "true" || tag === "false") return tag === "true";
      if (tag === "string" || tag === "data" || tag === "date") return "";
      if (tag === "dict") return {};
      if (tag === "array") return [];
      throw new Error(`plist: unexpected <${tag}/>`);
    }
    switch (tag) {
      case "plist": {
        const v = value();
        skipText();
        i++;
        return v;
      }
      case "dict": {
        const out: Record<string, PlistValue> = {};
        for (;;) {
          skipText();
          const k = tokens[i];
          if (k && k[1] === "/" && k[2] === "dict") {
            i++;
            return out;
          }
          if (!k || k[2] !== "key") throw new Error("plist: expected <key>");
          i++;
          const key = text("key");
          out[key] = value();
        }
      }
      case "array": {
        const out: PlistValue[] = [];
        for (;;) {
          skipText();
          const k = tokens[i];
          if (k && k[1] === "/" && k[2] === "array") {
            i++;
            return out;
          }
          out.push(value());
        }
      }
      case "integer":
      case "real":
        return Number(text(tag));
      case "string":
      case "date":
      case "data":
        return text(tag).trim();
      default:
        throw new Error(`plist: unexpected <${tag}>`);
    }
  };
  return value();
}

/**
 * An AppKit frame string ("x y w h screenX screenY screenW screenH", origin
 * at the bottom left of the primary display) as Electron bounds (origin at
 * its top left). window.ts still checks that they fit a connected display.
 */
export function boundsFromFrame(frame: string, primaryHeight: number): WindowBounds | undefined {
  const n = frame.trim().split(/\s+/).map(Number);
  if (n.length < 4 || n.slice(0, 4).some((v) => !Number.isFinite(v))) return undefined;
  const [x, y, width, height] = n.slice(0, 4).map(Math.round) as [number, number, number, number];
  if (width < 200 || height < 200 || primaryHeight <= 0) return undefined;
  return { x, y: primaryHeight - (y + height), width, height };
}

export interface MigrationInput {
  home: string;
  userData: string;
  settings: Settings;
  /** `defaults export com.trycua.spaces.macos -`, or null when the domain is empty or unreadable. */
  defaultsXml: string | null;
  primaryHeight: number;
  now: Date;
}

/** Copies the Swift app's files and returns the settings patch (with the marker), or null when already done. */
export function migrateFromSwiftApp(input: MigrationInput): Partial<Settings> | null {
  if (input.settings.swiftMigration) return null;
  const marker: SwiftMigration = { at: input.now.toISOString(), found: false, copied: [] };
  const patch: Partial<Settings> = { swiftMigration: marker };

  const from = swiftSupportDir(input.home);
  const to = appCoreStatePaths(input.userData);
  for (const [name, dest] of [
    ["settings.json", to.settings],
    ["onboarding.json", to.onboarding],
  ] as const) {
    const src = path.join(from, name);
    if (!existsSync(src)) continue;
    marker.found = true;
    if (existsSync(dest)) continue;
    mkdirSync(path.dirname(dest), { recursive: true });
    copyFileSync(src, dest);
    marker.copied.push(name);
  }

  try {
    const core = JSON.parse(readFileSync(path.join(from, "settings.json"), "utf8")) as { updateChannel?: unknown; lastSeenVersion?: unknown };
    if (typeof core.lastSeenVersion === "string") marker.from = core.lastSeenVersion;
    if ((core.updateChannel === "beta" || core.updateChannel === "stable") && !input.settings.updateChannel) {
      patch.updateChannel = core.updateChannel;
    }
  } catch {
    // No Swift settings, or damaged: the app core would have used its defaults too.
  }

  let defaults: Record<string, PlistValue> = {};
  if (input.defaultsXml) {
    try {
      const parsed = parsePlist(input.defaultsXml);
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) defaults = parsed;
    } catch {
      // Unreadable: keep what the files gave.
    }
  }
  // Keys only the Swift app writes (AppKit and Chromium share the domain now).
  if (Object.keys(defaults).some((k) => k === SWIFT_FRAME_KEY || /^(WebUI|SU)/.test(k))) marker.found = true;
  const frame = defaults[SWIFT_FRAME_KEY];
  if (typeof frame === "string" && !input.settings.windowBounds) {
    const bounds = boundsFromFrame(frame, input.primaryHeight);
    if (bounds) patch.windowBounds = bounds;
  }
  const bool = (key: string) => {
    const v = defaults[key];
    return typeof v === "boolean" ? v : typeof v === "number" ? v !== 0 : undefined;
  };
  const checks = bool("SUEnableAutomaticChecks");
  const downloads = bool("SUAutomaticallyUpdate");
  if (checks !== undefined) marker.sparkleAutomaticChecks = checks;
  if (downloads !== undefined) marker.sparkleAutomaticDownloads = downloads;
  return patch;
}
