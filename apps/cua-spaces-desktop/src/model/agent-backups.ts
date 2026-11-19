// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Agent setup keeps a copy of a config file the first time it edits it
// (`<file>.cua-backup-<time>`). Once Cua is taken out of an app's settings the
// copy has no use, and it only clutters the person's home folder, so removal
// deletes them. Only files Cua named this way, next to a config file Cua edits.
import { existsSync, mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

/** `config.toml.cua-backup-20261025T101500Z`: the base name, then the marker and a time. */
const BACKUP = /\.cua-backup-[0-9A-Za-z._-]+$/;

/** Deletes the backups Cua made of `config`; the files removed. */
export function removeBackups(config: string): string[] {
  const dir = path.dirname(config);
  const base = path.basename(config);
  let names: string[];
  try {
    names = readdirSync(dir);
  } catch {
    return [];
  }
  const gone: string[] = [];
  for (const name of names) {
    if (!name.startsWith(`${base}.cua-backup-`) || !BACKUP.test(name)) continue;
    try {
      rmSync(path.join(dir, name), { force: true });
      gone.push(path.join(dir, name));
    } catch {
      // A file we can't delete stays; nothing depends on it.
    }
  }
  return gone;
}

/**
 * The one-time sweep of the leftovers earlier versions left (every config file
 * Cua edits, whether or not Cua is still in it): done once per install, kept
 * in a marker file in the cua folder.
 */
export function sweepBackupsOnce(configs: string[], home: string = os.homedir()): string[] {
  const marker = path.join(home, ".cua", "agent-backups-swept");
  if (existsSync(marker)) return [];
  const gone = configs.flatMap(removeBackups);
  try {
    mkdirSync(path.dirname(marker), { recursive: true });
    writeFileSync(marker, new Date().toISOString());
  } catch {
    // The sweep runs again next time; it only deletes what is named above.
  }
  return gone;
}
