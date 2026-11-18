// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The core's `githubConnection.view` (`cua-spaces-app-core/src/github_connection.rs`), as the page draws it. */

export type GithubTone = "ok" | "quiet" | "attention";

export interface GithubRow {
  host: string;
  /** "GitHub", or the Enterprise host. */
  title: string;
  /** The state in plain words. */
  line: string;
  tone: GithubTone;
  /** The token can do more than a push and a pull request need (not shown when the host is off). */
  warning: string | null;
}

export interface GithubAccountRow {
  login: string;
  /** "gh's own account". */
  tag: string | null;
  selected: boolean;
  /** "Sign in again with gh auth login". */
  note: string | null;
}

export interface GithubSourceLine {
  id: string;
  line: string;
  /** "In use". */
  tag: string | null;
}

export interface GithubDetail {
  host: string;
  title: string;
  line: string;
  warning: string | null;
  switchLabel: string;
  switchHelp: string;
  enabled: boolean;
  /** The picker: only when gh has several accounts on the host. */
  accountsLabel: string | null;
  accountsHelp: string | null;
  accounts: GithubAccountRow[];
  /** "Use gh's own account", to undo a choice. */
  autoLabel: string | null;
  sourcesTitle: string;
  sources: GithubSourceLine[];
  /** What to run when nothing is signed in. */
  help: string | null;
  rescanLabel: string;
  /** "Paste a token" (github.com, when one can be saved). */
  pasteLabel: string | null;
  closeLabel: string;
}

export interface GithubView {
  title: string;
  footer: string;
  rows: GithubRow[];
  details: GithubDetail[];
  rescanLabel: string;
  notice: string | null;
}
