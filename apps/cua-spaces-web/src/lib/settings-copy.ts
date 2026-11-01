// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Plain words for Settings: the few labels the pages say in their own words.
 */

/** Launch at login, General. */
export const LAUNCH_LABEL = "Start Cua Spaces when you sign in";
export const LAUNCH_HELP = "Keeps your machines reachable after a restart.";

/** The core's "Last check: 2 minutes ago" as "Last checked 2 minutes ago" (About). */
export const lastChecked = (line: string) => line.replace(/^Last check:\s*/, "Last checked ");

/** "S3-compatible" is jargon: your own storage (S3). */
export const plainStorage = (text: string) => text.replace(/S3-compatible( bucket| storage)?/gi, "Your own storage (S3)");
