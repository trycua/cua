// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { VaultApp, VaultFiles, VaultRow, VaultSite, VaultView } from "@/bridge/contracts/keyvault";

/** One line of the flattened vault list: an app, a site, the files of an app
 * or an item (the SwiftUI list's `VaultLine`). The core decides what is open. */
export type VaultLine =
  | { type: "app"; key: string; app: VaultApp }
  | { type: "site"; key: string; app: VaultApp; site: VaultSite }
  | { type: "files"; key: string; app: VaultApp; files: VaultFiles }
  | { type: "item"; key: string; row: VaultRow; indent: number };

/** The core's `VaultView` as rows for a virtualized list: each app, and under
 * the open ones their sites (and a site's items when open) and files. */
export function vaultLines(view: VaultView): VaultLine[] {
  const out: VaultLine[] = [];
  for (const app of view.apps) {
    out.push({ type: "app", key: `app:${app.key}`, app });
    if (!app.open) continue;
    for (const site of app.sites) {
      out.push({ type: "site", key: `site:${site.key}`, app, site });
      if (site.open) for (const row of site.rows) out.push({ type: "item", key: `item:${row.id}`, row, indent: 2 });
    }
    if (app.files) {
      out.push({ type: "files", key: `files:${app.files.key}`, app, files: app.files });
      if (app.files.open) for (const row of app.files.rows) out.push({ type: "item", key: `item:${row.id}`, row, indent: 2 });
    }
  }
  return out;
}
