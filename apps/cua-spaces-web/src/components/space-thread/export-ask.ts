// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The card for an export the host asks about, or null for any other approval. */
export function exportAskText(action: string, details: string): { title: string; detail: string; allow: string; account: string | null } | null {
  if (!action.startsWith("export:")) return null;
  const known: Record<string, { title: string; allow: string }> = {
    "export:repo.push": { title: "Push to GitHub", allow: "Push and open" },
  };
  const k = known[action] ?? { title: "Send out of this Space", allow: "Allow" };
  return { ...k, detail: details.trim(), account: exportAccount(details) };
}

/** The sign-in the host will push with, as it names it in the card's details ("Cua pushes it from this computer as ada, signed in with gh;"), or null. A plain string for the card model; the layout is not decided here. */
export function exportAccount(details: string): string | null {
  const m = /pushes it from this computer as (.+?); the Space holds no GitHub access/.exec(details);
  return m ? m[1]!.trim() : null;
}
