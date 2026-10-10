// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** "Added Oct 4, 2026": the core's label and the date a key was added. */
export const addedText = (label: string, ms: number, locale?: string, timeZone?: string) =>
  `${label} ${new Date(ms).toLocaleDateString(locale ?? "en-US", { month: "short", day: "numeric", year: "numeric", timeZone })}`;

/** Whether an agent's error is a missing provider key, which Settings → Agents fixes. */
export const needsAgentKey = (error: string | null | undefined): boolean =>
  Boolean(error && /Settings → Agents|Authentication required|has no credential/i.test(error));
