// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Where an update is, for the in-app indicator (the page's `update.get`).
// A pure state machine over what electron-updater reports, kept free of
// Electron so it can be tested on its own. The idea is T3 Code's updater
// (github.com/pingdotgg/t3code, apps/desktop/src/updates/updateMachine.ts):
// a downloaded update is never lost to a later check, a failed background
// step is quiet, and nothing installs without the user's click.

export type UpdateStatus = "idle" | "checking" | "available" | "downloading" | "downloaded" | "error";

/** The `UpdateState` the page reads (apps/cua-spaces-web/src/bridge/contracts/settings.ts). */
export interface UpdateStateValue {
  status: UpdateStatus;
  /** The version this app runs. */
  currentVersion: string;
  /** The version found, downloading or ready; null with none. */
  version: string | null;
  /** 0 to 100 while downloading; else null. */
  percent: number | null;
  /** The GitHub release notes page for `version`. */
  notesUrl: string | null;
  /** Why the last step failed (`error` only). */
  message: string | null;
}

/** A background check waits this long after launch, so it never competes with starting up. */
export const UPDATE_STARTUP_DELAY_MS = 15_000;
/** And repeats at this pace while the app stays open. */
export const UPDATE_POLL_MS = 4 * 60 * 60 * 1000;

const RELEASES = "https://github.com/trycua/cua/releases/tag";

/** The release notes for a version: the repository's `cua-spaces-v<version>` tag. */
export function releaseNotesUrl(version: string): string {
  return `${RELEASES}/cua-spaces-v${encodeURIComponent(version.trim())}`;
}

export const initialUpdateState = (currentVersion: string): UpdateStateValue => ({
  status: "idle",
  currentVersion,
  version: null,
  percent: null,
  notesUrl: null,
  message: null,
});

export type UpdateEvent =
  | { type: "checking" }
  | { type: "available"; version: string }
  | { type: "progress"; percent: number }
  | { type: "downloaded"; version: string }
  | { type: "none" }
  | { type: "error"; message: string };

const downloadedState = (s: UpdateStateValue, version: string): UpdateStateValue => ({
  ...s,
  status: "downloaded",
  version,
  percent: 100,
  notesUrl: releaseNotesUrl(version),
  message: null,
});

/** The next state. A downloaded update stays downloaded through later checks and failures. */
export function reduceUpdate(s: UpdateStateValue, e: UpdateEvent): UpdateStateValue {
  const downloaded = s.status === "downloaded" && s.version !== null;
  switch (e.type) {
    case "checking":
      // A found or running update is not hidden by the next check.
      return s.status === "idle" || s.status === "error" ? { ...s, status: "checking", message: null } : s;
    case "available":
      if (downloaded && s.version === e.version) return s;
      // A newer one than the one downloaded replaces it: its download starts now.
      return { ...s, status: "available", version: e.version, percent: null, notesUrl: releaseNotesUrl(e.version), message: null };
    case "progress":
      if (downloaded) return s;
      return { ...s, status: "downloading", percent: Math.max(0, Math.min(100, e.percent)), message: null };
    case "downloaded":
      return downloadedState(s, e.version);
    case "none":
      return downloaded ? s : { ...initialUpdateState(s.currentVersion) };
    case "error":
      // The downloaded update is still good; a failed check or download is the next poll's to retry.
      return downloaded ? s : { ...s, status: "error", percent: null, message: e.message };
  }
}
