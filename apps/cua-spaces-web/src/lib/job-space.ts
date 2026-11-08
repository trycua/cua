// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A job's Space as a row in the child Spaces list: ONE row for the whole life of the job (its Space and
 * its run are the same row), a GPU icon in place of the OS icon when it has a GPU, a name in the shape
 * of a pet name (never an id), and a JOB tag. The host labels the Space it makes for a job:
 * `cua.job`, `cua.job.name`, `cua.job.gpu`. The name is cut again here, so a label from an older or
 * another host never shows an id.
 */

export const JOB_LABELS = { job: "cua.job", name: "cua.job.name", gpu: "cua.job.gpu" } as const;

/** What the row shows for a job. */
export interface JobRow {
  /** The name to show: a pet-name shape (`logo-grower`), never an id. */
  name: string;
  /** A GPU job: the row shows the GPU icon; otherwise the OS icon like any child. */
  gpu: boolean;
}

/** A job name in the shape of a pet name: lower case words of letters and digits joined by single hyphens, at most 24 characters; `null` when nothing is left or it is an id (`p-train-1a2b3c4d`, `cj-…-1`). Same rule as the host's. */
export function sanitizeJobName(raw: string): string | null {
  const words = raw
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 24)
    .replace(/-+$/g, "");
  if (!words) return null;
  const parts = words.split("-");
  const hex = (x: string, n: number) => x.length >= n && /^[0-9a-f]+$/.test(x);
  const idLike = parts.some((x) => hex(x, 8)) || (["p", "l", "cj", "j"].includes(parts[0] ?? "") && parts.some((x) => hex(x, 6)));
  return idLike ? null : words;
}

/** The row for a Space's labels, or `null` when it is not a job's Space. A job without a usable name shows a neutral one, never its id. */
export function jobRow(labels: Readonly<Record<string, string>> | undefined): JobRow | null {
  if (!labels || labels[JOB_LABELS.job] !== "1") return null;
  return { name: sanitizeJobName(labels[JOB_LABELS.name] ?? "") ?? "job", gpu: labels[JOB_LABELS.gpu] === "1" };
}

/** What a Space says of itself when it is a job's (the wire's `jobName`, `jobGpu`, `jobRun`), or `null`. */
export interface JobFields {
  jobName?: string;
  jobGpu?: boolean;
  jobRun?: string;
}

/** The row for a Space's job fields; a job without a usable name shows a neutral one, never its id. */
export function jobOfSpace(s: JobFields): JobRow | null {
  if (s.jobName === undefined) return null;
  return { name: sanitizeJobName(s.jobName) ?? "job", gpu: Boolean(s.jobGpu) };
}

/** One row per job: the Spaces that are jobs' collapse to the first Space of each run (`jobRun`, else the Space itself), so a job with several Spaces (attempts) is one row. */
export function oneRowPerJob<T extends { id: string } & JobFields>(spaces: readonly T[]): T[] {
  const seen = new Set<string>();
  const out: T[] = [];
  for (const s of spaces) {
    if (s.jobName !== undefined) {
      const k = s.jobRun || s.id;
      if (seen.has(k)) continue;
      seen.add(k);
    }
    out.push(s);
  }
  return out;
}
