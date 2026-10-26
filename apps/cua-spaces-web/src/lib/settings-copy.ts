// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Plain words for Settings. The app core still sends the old labels
 * (`enrolled`, `revoke`, `re-verify`); the Settings pages show these instead,
 * so a high-schooler can read them. One list, one place to change a word.
 */

const DEVICE_WORDS: readonly [RegExp, string][] = [
  [/^Enroll…?$/, "Trust this computer"],
  [/^Enroll This Mac…?$/, "Trust this computer"],
  [/^Re-verify This Mac…?$/, "Trust this computer"],
  [/^Enrolled until\b/, "Trusted until"],
  [/^Enrolled$/, "Trusted"],
  [/^Needs enrollment\b/, "Not trusted yet"],
  [/^Re-verification due$/, "Needs a quick check"],
  [/^Revoked$/, "Removed"],
  [/\bre-verify in\b/, "check again in"],
  [/\bRe-verification due\b/, "Needs a quick check"],
  [/\bEnrolled\b/, "Trusted"],
  [/^Revoke…?$/, "Remove"],
  [/^Revoke$/, "Remove"],
  [/^This Device$/, "This computer"],
  [/^Your Devices$/, "Your computers"],
  [/^Recent Access$/, "Recent activity"],
  [/^No access yet$/, "No activity yet"],
  [/^New Computers$/, "Computers waiting for you"],
];

/** One Devices label or detail line in plain words; anything it does not know stays as it is. */
export function plainDevice(text: string): string {
  return DEVICE_WORDS.reduce((out, [from, to]) => out.replace(from, to), text);
}

const SENTENCE_WORDS: readonly [RegExp, string][] = [
  [/\bunenrolled\b/g, "untrusted"],
  [/\bEnroll\b/g, "Trust"],
  [/\benroll\b/g, "trust"],
  [/\benrolled\b/g, "trusted"],
  [/\bEnrolled\b/g, "Trusted"],
  [/\bRe-verification is due\b/g, "It needs a quick check"],
  [/\bRe-verify\b/g, "Check"],
  [/\bre-verify\b/g, "check"],
  [/\bRevoke\b/g, "Remove"],
  [/\brevoke\b/g, "remove"],
  [/\brevoked\b/g, "removed"],
  [/\bThis Device\b/g, "This computer"],
  [/\bThis device\b/g, "This computer"],
  [/\bthis device\b/g, "this computer"],
];

/** A whole sentence (a banner, a dialog, an activity line) in plain words. */
export function plainDeviceSentence(text: string): string {
  return SENTENCE_WORDS.reduce((out, [from, to]) => out.replace(from, to), plainDevice(text));
}

/** The keys page's line under the list, with "provider key" as "account key from their AI company". */
export function plainKeysIntro(intro: string): string {
  return intro.replace("need a provider key to run.", "need an account key from their AI company to run.");
}

/** Settings, Connected apps, Account keys. */
export const KEYS_TITLE = "Account keys";
export const KEYS_ADD_OTHER_LABEL = "Add a key for another AI company";
export const KEYS_ADD_OTHER_BUTTON = "Add key";
export const KEYS_OTHER_HELP = "Use the name its app expects, like GEMINI_API_KEY.";

/** Launch at login, General. */
export const LAUNCH_LABEL = "Start Cua Spaces when you sign in";
export const LAUNCH_HELP = "Keeps your computers reachable after a restart.";

/** The core's "Last check: 2 minutes ago" as "Last checked 2 minutes ago" (About). */
export const lastChecked = (line: string) => line.replace(/^Last check:\s*/, "Last checked ");

/** "S3-compatible" is jargon: your own storage (S3). */
export const plainStorage = (text: string) => text.replace(/S3-compatible( bucket| storage)?/gi, "Your own storage (S3)");
