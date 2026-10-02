// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { core } from "../core";

/** Paths as native macOS apps show them: under the home folder as `~/...`. */
export function displayPath(path: string, home: string | null | undefined): string {
  return core("paths.displayPath", { path, home: home ?? null });
}

/** Replaces every occurrence of the home folder in free text. */
export function displayPaths(text: string, home: string | null | undefined): string {
  return core("paths.displayPaths", { text, home: home ?? null });
}
