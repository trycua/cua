// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The preload's part of the drop well (bridge/files.ts): a request for
// `spaces.droppedFiles` carries the last drop's paths, as the preload read
// them from the drop, in place of anything the page sent. No Electron here.

export function withDroppedPaths(request: unknown, paths: readonly string[]): unknown {
  const r = request as { method?: unknown; args?: unknown } | null;
  if (!r || typeof r !== "object" || r.method !== "spaces.droppedFiles") return request;
  const args = r.args && typeof r.args === "object" && !Array.isArray(r.args) ? (r.args as Record<string, unknown>) : {};
  return { ...r, args: { ...args, paths: [...paths] } };
}
