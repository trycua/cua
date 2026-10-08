// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { VaultApp, VaultRow, VaultView } from "@/bridge/contracts/keyvault";
import { vaultLines } from "./keyvault";

const row = (id: string): VaultRow => ({
  id,
  kind: "cookie",
  kindLabel: "Cookie",
  symbol: "circle.hexagongrid.fill",
  title: id,
  subtitle: "",
  updated: "now",
  locked: true,
  lockSymbol: "lock.fill",
  lockHelp: "",
  identityProvider: false,
  selected: false,
});

const group = { count: 1, selected: "off" as const, lock: "locked" as const, unlockIds: [] as string[], lockIds: [] as string[] };
const app = (key: string, open: boolean, siteOpen: boolean, files: boolean): VaultApp => ({
  ...group,
  key,
  providerId: key,
  name: key,
  summary: "",
  updated: "",
  open,
  sites: [{ ...group, key: `${key}/github.com`, site: "github.com", counts: "", updated: "", open: siteOpen, rows: [row(`${key}-c1`)] }],
  files: files ? { ...group, key: `${key}/files`, open: true, rows: [row(`${key}-f1`)] } : null,
});

describe("vaultLines", () => {
  it("lists each app, and under an open one its sites, their open items and its files", () => {
    const view = { apps: [app("chrome", true, true, true), app("safari", false, true, true), app("slack", true, false, false)] } as VaultView;
    expect(vaultLines(view).map((l) => l.key)).toEqual([
      "app:chrome",
      "site:chrome/github.com",
      "item:chrome-c1",
      "files:chrome/files",
      "item:chrome-f1",
      "app:safari",
      "app:slack",
      "site:slack/github.com",
    ]);
    expect(vaultLines({ apps: [] } as unknown as VaultView)).toEqual([]);
  });
});
