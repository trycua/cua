// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** Who's on a Space or a computer, in words and avatars: the names line, an avatar's letter and its colour. */

import { t } from "@/i18n";

/** "Sam", "Sam and Priya", "Sam, Priya and 3 others". */
export function namesText(list: readonly string[]): string {
  if (list.length === 2) return t("computers.tile.names_two", { first: list[0], second: list[1] });
  if (list.length < 2) return list.join("");
  return t("computers.tile.names_many", { first: list[0], second: list[1], count: list.length - 2 });
}

/** An avatar's letter: the first letter of a name or address. */
export function initial(name: string): string {
  return (name.trim()[0] ?? "?").toUpperCase();
}

const AVATAR_COLORS = ["bg-violet-500", "bg-amber-600", "bg-teal-600", "bg-sky-600", "bg-rose-500", "bg-emerald-600", "bg-indigo-500", "bg-orange-500"];

/** The same person or team gets the same color everywhere. */
export function avatarColor(key: string): string {
  let h = 0;
  for (const c of key.toLowerCase()) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return AVATAR_COLORS[h % AVATAR_COLORS.length]!;
}
