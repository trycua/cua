// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Vitest reporter for CI flaky-test handling (.github/scripts/flaky/flaky.py).
// Writes one row per test to $FLAKY_RESULTS_OUT: "flaky" when the test passed
// only on a retry, "failed" when it failed after its retries. Does nothing
// when FLAKY_RESULTS_OUT is unset, so local runs are unchanged.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, relative } from "node:path";

export default class FlakyReporter {
  rows = [];

  onTestCaseResult(testCase) {
    const result = testCase.result();
    if (result.state !== "passed" && result.state !== "failed") return;
    const retried = (testCase.diagnostic()?.retryCount ?? 0) > 0;
    const status = result.state === "failed" ? "failed" : retried ? "flaky" : "passed";
    if (status === "passed") return;
    const file = relative(process.cwd(), testCase.module.moduleId);
    this.rows.push({
      id: `${file} > ${testCase.fullName}`,
      status,
      message: String(result.errors?.[0]?.message ?? "").slice(0, 1500),
    });
  }

  onTestRunEnd() {
    const out = process.env.FLAKY_RESULTS_OUT;
    if (!out) return;
    mkdirSync(dirname(out), { recursive: true });
    writeFileSync(out, JSON.stringify(this.rows));
  }
}
