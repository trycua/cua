// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** "macos-1", "linux-2": the first name of that system's not taken yet
 * (Space names are DNS labels). */
export function extraName(os: "macos" | "linux" | "omarchy", taken: { name: string }[]): string {
  const names = new Set(taken.map((t) => t.name.toLowerCase()));
  let n = 1;
  while (names.has(`${os}-${n}`)) n++;
  return `${os}-${n}`;
}

/** How the card says a name: "macos-1" is "macOS 1". */
export function spokenName(name: string): string {
  const m = /^(macos|linux|omarchy)-(\d+)$/.exec(name);
  const spoken: Record<string, string> = { macos: "macOS", linux: "Linux", omarchy: "Omarchy" };
  return m ? `${spoken[m[1]!]} ${m[2]}` : name;
}
