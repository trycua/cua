// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A failed "Set up for access" or This machine button in words a person can
// act on (the SwiftUI app's HostSetupFailure.swift): a short title, one
// plain message (never a URL or a raw error), and the raw error kept as is
// for the Details disclosure. On Windows and Linux the words name the
// computer as the page does ("this PC", "this computer") and the system
// that runs the host service.
import type { PresentedFailure } from "../bridge/host";

export type HostFailureKind = "download" | "network" | "throttled" | "guiSession" | "signedOut" | "service" | "other";

export interface HostSetupFailure extends PresentedFailure {
  kind: HostFailureKind;
  message: string;
}

export const RETRY_LABEL = "Retry";
export const SIGN_IN_LABEL = "Sign In";

/** What the app calls the computer it runs on, mid-sentence. */
export function thisComputer(platform: NodeJS.Platform): string {
  if (platform === "darwin") return "this Mac";
  return platform === "win32" ? "this PC" : "this computer";
}

const SYSTEM: Partial<Record<NodeJS.Platform, string>> = { darwin: "macOS", win32: "Windows", linux: "Linux" };

/**
 * Seconds to wait when the raw error is the server telling us to slow down
 * (the host's own sentence, or a bare HTTP 429 "Too Many Requests" from an
 * older host): a number, null for "unknown", undefined when it is not that.
 * Only the generic throttle: a 429 that says why (a limit, a quota) is not.
 */
export function throttledWait(raw: string): number | null | undefined {
  const own = /too many attempts right now\. wait (\d+) seconds?/i.exec(raw);
  if (own) return Number(own[1]);
  if (/\bhttp 429\b[:\s]*too many requests\s*$/i.test(raw.trim())) return null;
  return undefined;
}

/** Which kind of failure the raw setup error is. */
export function classify(raw: string): HostFailureKind {
  if (throttledWait(raw) !== undefined) return "throttled";
  const s = raw.toLowerCase();
  const any = (needles: string[]) => needles.some((n) => s.includes(n));
  const word = (w: string) => new RegExp(`\\b${w}\\b`).test(s);

  const downloading = any(["download:", "download failed", "cua-spacesd"]);
  const unavailable = word("404") || any(["not found", "no published", "not available", "unavailable"]);
  if (downloading && unavailable) return "download";

  if (
    any([
      "timed out", "timeout", "offline", "not connected to the internet", "network is unreachable",
      "network unreachable", "connection refused", "connection reset", "connection closed",
      "could not resolve", "couldn't resolve", "failed to lookup", "dns error", "dns lookup",
      "error sending request", "host is unreachable", "no route to host", "tls handshake",
    ])
  )
    return "network";

  if (any(["aqua", "no gui session", "gui session", "not logged in at the console", "console user", "no console"])) return "guiSession";

  if (any(["launchctl", "launchd", "host service", "bootstrap failed", "schtasks", "systemctl"])) return "service";

  if (
    word("401") ||
    any([
      "signed out", "not signed in", "sign in again", "please sign in", "unauthenticated", "unauthorized",
      "login required", "not logged in", "session expired", "token expired", "invalid token",
      "no account token", "missing account token",
    ])
  )
    return "signedOut";

  if (any(["download:", "download failed"])) return "download";
  return "other";
}

/** The title and message for a kind. */
export function failureWords(kind: HostFailureKind, platform: NodeJS.Platform = "darwin", raw = ""): [string, string] {
  const here = thisComputer(platform);
  const system = SYSTEM[platform] ?? "The system";
  const remote = platform === "darwin" ? "Screen Sharing" : "Remote Desktop";
  switch (kind) {
    case "download":
      return [
        "Couldn’t download the Cua host service",
        `The service ${here} needs to accept connections isn’t available right now. ` +
          "Check your internet connection and try again; if it keeps failing, update Cua Spaces.",
      ];
    case "network":
      return ["Couldn’t connect", "Cua Spaces couldn’t reach the internet. Check your connection and try again."];
    case "throttled": {
      const secs = throttledWait(raw);
      return [
        "Too many attempts right now",
        secs ? `Trying again in ${secs} ${secs === 1 ? "second" : "seconds"}.` : "Wait a few seconds, then try again.",
      ];
    }
    case "guiSession":
      return [
        `Sign in to ${here} first`,
        `Setting up access needs someone signed in at ${here}’s own screen, not only over ` +
          `SSH or ${remote}. Sign in at ${platform === "darwin" ? "the Mac" : "the computer"}, then try again.`,
      ];
    case "signedOut":
      return ["Sign in to Cua", `Your Cua account isn’t signed in on ${here}. Sign in, then try again.`];
    case "service":
      return [
        "Couldn’t start the Cua host service",
        `${system} didn’t start the service your other devices connect to. Try again; ` +
          (platform === "win32"
            ? "if it keeps failing, open Cua Spaces as administrator (right-click it, Run as administrator) and set up access once more."
            : `if it keeps failing, restart ${here}.`),
      ];
    case "other":
      return [`Couldn’t set up ${here} for access`, "Something went wrong. Try again, or open Details to see what happened."];
  }
}

/** The failure for a raw error (its kind read from the words, or given: the account refused, whatever its words). */
export function presentHostFailure(raw: string, platform: NodeJS.Platform = "darwin", kind: HostFailureKind = classify(raw)): HostSetupFailure {
  const [title, message] = failureWords(kind, platform, raw);
  return { kind, title, message, details: raw, actionLabel: kind === "signedOut" ? SIGN_IN_LABEL : RETRY_LABEL };
}
