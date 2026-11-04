// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The opt-in diagnostic report for a failed create (the app core's
// `diagnostics` module). The core builds and redacts it; the review sheet
// lets the person edit or leave out each part; Send opens a prefilled email
// to Cua support in their mail app with what they kept. Reports are private:
// nothing is sent before they press Send there, and it goes only to support.

import type { CoreClient } from "./core";
import type { AboutInput } from "./contracts/settings";

/** What the page knows about one failed create (`CreateReportInput`). */
export interface CreateReportInput {
  error: string;
  shown?: string | null;
  phase: string;
  os: string;
  provider: string;
  kind?: string | null;
  image?: string | null;
  gpu: boolean;
  elapsedMs?: number | null;
  about?: AboutInput | null;
  names: string[];
}

/** One part the person can edit or leave out. */
export interface ReportPart {
  id: string;
  label: string;
  text: string;
}

/** The review sheet (`CreateReport`). */
export interface CreateReport {
  title: string;
  intro: string;
  subject: string;
  parts: ReportPart[];
  sendLabel: string;
  errorKind: string;
}

/** The parts the person kept, as they edited them. */
export interface ReportSend {
  subject: string;
  parts: ReportPart[];
}

type Json = Parameters<CoreClient["tryCall"]>[1];

export const createReport = (core: CoreClient, input: CreateReportInput): CreateReport | null =>
  core.tryCall<CreateReport>("diagnostics.createReport", { input } as unknown as Json) ?? null;

/** The `mailto:` link to Cua support with the kept parts, redacted again. */
export const reportMailto = (core: CoreClient, send: ReportSend, names: string[]): string | null =>
  core.tryCall<string>("diagnostics.reportMailto", { send, names } as unknown as Json) ?? null;
