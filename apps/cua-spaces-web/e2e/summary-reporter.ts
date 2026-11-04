// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Prints the parity table (flow, status, reason) after a run and writes it
 * to test-results/parity-summary.md. Native flows (skipped here with a
 * `native` annotation; the Swift parity runner covers them) count apart
 * from skipped ones.
 */

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import type { Reporter, TestCase, TestResult } from "@playwright/test/reporter";

interface Row {
  flow: string;
  status: "passed" | "failed" | "native" | "skipped";
  reason: string;
}

const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "test-results", "parity-summary.md");

const oneLine = (s: string) =>
  s
    // Playwright's messages carry ANSI colours.
    .replace(/\u001b\[[0-9;]*m/g, "")
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean)
    .slice(0, 2)
    .join(" ")
    .replace(/\|/g, "\\|")
    .slice(0, 160);

export default class ParitySummary implements Reporter {
  private rows = new Map<string, Row>();

  onTestEnd(test: TestCase, result: TestResult): void {
    if (test.parent.title !== "parity flows") return;
    const note = (type: string) => test.annotations.find((a) => a.type === type)?.description ?? "";
    const native = test.annotations.some((a) => a.type === "native");
    const status: Row["status"] =
      result.status === "skipped" ? (native ? "native" : "skipped") : result.status === "passed" ? "passed" : "failed";
    const reason =
      status === "native"
        ? `${note("native")} (covered by the Swift parity runner)`
        : status === "skipped"
          ? note("skip")
          : status === "failed"
            ? oneLine(result.error?.message ?? "failed")
            : note("parity");
    // Retries overwrite: the last attempt is the one that counts.
    this.rows.set(test.title, { flow: test.title, status, reason });
  }

  onEnd(): void {
    if (this.rows.size === 0) return;
    const order = { failed: 0, passed: 1, native: 2, skipped: 3 };
    const rows = [...this.rows.values()].sort((a, b) => order[a.status] - order[b.status] || a.flow.localeCompare(b.flow));
    const count = (s: Row["status"]) => rows.filter((r) => r.status === s).length;
    const table = [
      `Parity: ${count("passed")} passed, ${count("failed")} failed, ${count("native")} native by design, ${count("skipped")} skipped of ${rows.length} flows`,
      "",
      `Covered: ${count("passed") + count("native")} of ${rows.length} (passed in the web UI, or native by design and covered by the Swift parity runner).`,
      "",
      "| Flow | Status | Reason |",
      "|---|---|---|",
      ...rows.map((r) => `| ${r.flow} | ${r.status} | ${r.reason} |`),
      "",
    ].join("\n");
    console.log(`\n${table}`);
    mkdirSync(dirname(OUT), { recursive: true });
    writeFileSync(OUT, table);
  }

  printsToStdio(): boolean {
    return true;
  }
}
