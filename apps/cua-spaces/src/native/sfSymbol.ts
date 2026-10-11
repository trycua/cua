// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Fetch a macOS SF Symbol rendered to a PNG data URL by the shell. Returns null
 * off the native macOS shell or if the symbol can't be rendered, so callers fall
 * back to an inline SVG glyph. The PNG is a template (alpha = shape); render it
 * as a CSS mask over `currentColor` to theme it.
 */
export async function sfSymbol(name: string, size = 14): Promise<string | null> {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    return await invoke<string>("sf_symbol", { name, size });
  } catch {
    return null;
  }
}
