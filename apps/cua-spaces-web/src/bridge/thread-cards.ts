// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The cards in a Space's thread, as the app core draws them
 * (`threadCards.exportCard`, `threadCards.author`;
 * `cua-spaces-app-core/src/thread_cards.rs`). Every word and number is the
 * core's; the page lays the pieces out and colours them. Without the core
 * there is no card: the approval shows its plain text.
 */

import type { CoreClient } from "./core";
import type { ThreadAuthor } from "./ops/thread";

export type Square = "add" | "del" | "none";

export interface CardFileRow {
  name: string;
  dir: string;
  status: string;
  additions: string;
  deletions: string;
  note: string;
  bar: [Square, Square, Square, Square, Square];
}

export interface CardCommitRow {
  sha: string;
  author: string;
  subject: string;
}

export interface ExportCardView {
  repo: string;
  /** The one header line: who asks to do what on which repository. */
  headline: string;
  branch: string;
  base: string | null;
  branchLine: string;
  /** The diff line: `5 files`, `+30`, `-16`. */
  totals: { files: string; additions: string; deletions: string };
  account: string | null;
  /** There is something behind `Show changes`. */
  expandable: boolean;
  showLabel: string;
  hideLabel: string;
  /** The first three files, the first three commits and, when a pull request will open, its preview. */
  files: CardFileRow[];
  moreFiles: string | null;
  commitsHeading: string;
  commits: CardCommitRow[];
  moreCommits: string | null;
  pr: { title: string; state: string; body: string[] } | null;
  /** `Push and open PR`, or `Push`. */
  allow: string;
  deny: string;
  resultApproved: string;
  resultDeclined: string;
}

export interface AuthorView {
  kind: string;
  name: string;
  /** The small tag next to the name (`APP`), on Cua's own entries only. */
  tag: string | null;
  /** The picture, as an id each shell maps to the app's icon (`cua-app-icon`); null keeps the thread's avatar. */
  avatar: string | null;
  avatarLabel: string | null;
}

/** The card for a push the host asks about; null without the core or for data that is not a card. */
export function exportCardView(core: CoreClient, card: unknown): ExportCardView | null {
  if (!card || typeof card !== "object") return null;
  return core.tryCall<ExportCardView>("threadCards.exportCard", { input: card }) ?? null;
}

/** How an author shows (the name, the APP tag and the app icon for Cua's own entries). */
export function authorView(core: CoreClient, author: Pick<ThreadAuthor, "kind" | "id" | "name">): AuthorView | null {
  return core.tryCall<AuthorView>("threadCards.author", { author }) ?? null;
}
