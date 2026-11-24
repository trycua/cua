// Vitest reporter for CI flaky-test handling (.github/scripts/flaky/flaky.py).
// Writes one row per test to $FLAKY_RESULTS_OUT: "flaky" when the test passed
// only on a retry, "failed" when it failed after its retries. Does nothing
// when FLAKY_RESULTS_OUT is unset, so local runs are unchanged. Also writes
// per-file test times next to it (<out>.timings.json), read by
// .github/scripts/flaky/timings.py through flaky.py.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, relative } from "node:path";

export default class FlakyReporter {
  rows = [];
  files = new Map();

  onTestCaseResult(testCase) {
    const result = testCase.result();
    const where = relative(process.cwd(), testCase.module.moduleId);
    const seen = this.files.get(where) ?? { file: where, seconds: 0, tests: 0 };
    seen.seconds += (testCase.diagnostic()?.duration ?? 0) / 1000;
    seen.tests += 1;
    this.files.set(where, seen);
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
    const timings = [...this.files.values()]
      .map((f) => ({ ...f, seconds: Math.round(f.seconds * 1000) / 1000 }))
      .sort((a, b) => b.seconds - a.seconds);
    writeFileSync(`${out}.timings.json`, JSON.stringify(timings));
  }
}
