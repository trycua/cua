// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Removing Cua from an app also deletes the `.cua-backup-*` copies setup made
// of its config, and only those.
import { mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { removeBackups, sweepBackupsOnce } from "../src/model/agent-backups";

const dirs: string[] = [];
const tmp = () => {
  const d = mkdtempSync(path.join(os.tmpdir(), "cua-backups-"));
  dirs.push(d);
  return d;
};
afterEach(() => {
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});

describe("removeBackups", () => {
  it("deletes Cua's copies of that config, and nothing else", () => {
    const d = tmp();
    const config = path.join(d, "config.toml");
    for (const f of ["config.toml", "config.toml.cua-backup-20261025T101500Z", "config.toml.cua-backup-1", "other.toml.cua-backup-1", "config.toml.bak", "config.toml.cua-backup"]) writeFileSync(path.join(d, f), "x");
    const gone = removeBackups(config);
    expect(gone.map((g) => path.basename(g)).sort()).toEqual(["config.toml.cua-backup-1", "config.toml.cua-backup-20261025T101500Z"]);
    expect(readdirSync(d).sort()).toEqual(["config.toml", "config.toml.bak", "config.toml.cua-backup", "other.toml.cua-backup-1"]);
  });

  it("is quiet when the folder is gone", () => {
    expect(removeBackups(path.join(tmp(), "nope", "config.json"))).toEqual([]);
  });
});

describe("sweepBackupsOnce", () => {
  it("clears the leftovers once per install", () => {
    const home = tmp();
    const config = path.join(home, ".claude.json");
    writeFileSync(`${config}.cua-backup-1`, "x");
    expect(sweepBackupsOnce([config], home)).toHaveLength(1);
    // A backup made after the sweep is left alone: the marker says it ran.
    writeFileSync(`${config}.cua-backup-2`, "x");
    expect(sweepBackupsOnce([config], home)).toEqual([]);
    expect(readdirSync(home)).toContain(".claude.json.cua-backup-2");
    mkdirSync(path.join(home, ".cua"), { recursive: true });
  });
});
